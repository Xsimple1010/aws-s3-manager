use aws_config::BehaviorVersion;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{ProvideCredentials, Region, SharedCredentialsProvider};
use aws_sdk_s3::error::DisplayErrorContext;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::json;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use aws_sdk_s3::operation::put_object::PutObjectOutput;

/// Validade da policy do presigned POST.
const UPLOAD_POST_TTL_MINUTES: i64 = 10;

/// Resultado do presigned POST: o front faz um `multipart/form-data` POST para `url`
/// com todos os `fields` e, por ÚLTIMO, o campo `file` com o conteúdo.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PresignedPost {
    pub url: String,
    pub fields: HashMap<String, String>,
}

#[derive(Clone)]
pub struct AwsS3Manager {
    bucket: Arc<String>,
    region: Arc<String>,
    credentials: SharedCredentialsProvider,
    client: Client,
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC aceita chave de qualquer tamanho");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

impl AwsS3Manager {
    pub async fn new(bucket: String, region: String) -> Self {
        // aws_config::defaults já usa a cadeia padrão de credenciais
        let config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region.clone()))
            .load()
            .await;

        // O provedor vem do SdkConfig: Config::credentials_provider() do aws_sdk_s3
        // está deprecated e sempre retorna None.
        let credentials = config
            .credentials_provider()
            .expect("aws_config::defaults sempre configura um provedor de credenciais");

        Self {
            bucket: Arc::new(bucket),
            region: Arc::new(region),
            credentials,
            client: Client::new(&config),
        }
    }

    /// Gera um presigned POST que só aceita:
    /// - exatamente a `key` informada
    /// - exatamente o `content_type` informado
    /// - arquivos entre 1 byte e `max_bytes`
    pub async fn create_upload_post(
        &self,
        key: String,
        content_type: &str,
        max_bytes: u64,
    ) -> Result<PresignedPost, String> {
        let creds = self
            .credentials
            .provide_credentials()
            .await
            .map_err(|e| format!("Não conseguimos obter as credenciais: {e}"))?;

        let now = chrono::Utc::now();
        let date = now.format("%Y%m%d").to_string();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let credential = format!(
            "{}/{}/{}/s3/aws4_request",
            creds.access_key_id(),
            date,
            self.region
        );
        let expiration = (now + chrono::Duration::minutes(UPLOAD_POST_TTL_MINUTES))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();

        let mut conditions = vec![
            json!({"bucket": self.bucket.as_str()}),
            json!({"key": key}),
            json!({"Content-Type": content_type}),
            json!(["content-length-range", 1, max_bytes]),
            json!({"x-amz-algorithm": "AWS4-HMAC-SHA256"}),
            json!({"x-amz-credential": credential}),
            json!({"x-amz-date": amz_date}),
        ];
        // Credenciais temporárias (role, SSO) exigem o token
        if let Some(token) = creds.session_token() {
            conditions.push(json!({"x-amz-security-token": token}));
        }

        let policy = json!({"expiration": expiration, "conditions": conditions}).to_string();
        let policy_b64 = STANDARD.encode(policy);

        // Cadeia de chaves SigV4
        let k_date = hmac_sha256(
            format!("AWS4{}", creds.secret_access_key()).as_bytes(),
            date.as_bytes(),
        );
        let k_region = hmac_sha256(&k_date, self.region.as_bytes());
        let k_service = hmac_sha256(&k_region, b"s3");
        let k_signing = hmac_sha256(&k_service, b"aws4_request");
        let signature = hex::encode(hmac_sha256(&k_signing, policy_b64.as_bytes()));

        let mut fields = HashMap::from([
            ("key".to_string(), key),
            ("Content-Type".to_string(), content_type.to_string()),
            ("policy".to_string(), policy_b64),
            ("x-amz-algorithm".to_string(), "AWS4-HMAC-SHA256".to_string()),
            ("x-amz-credential".to_string(), credential),
            ("x-amz-date".to_string(), amz_date),
            ("x-amz-signature".to_string(), signature),
        ]);
        if let Some(token) = creds.session_token() {
            fields.insert("x-amz-security-token".to_string(), token.to_string());
        }

        Ok(PresignedPost {
            url: format!("https://{}.s3.{}.amazonaws.com/", self.bucket, self.region),
            fields,
        })
    }

    /// PUT pré-assinado simples. ATENÇÃO: não limita tipo nem tamanho do arquivo.
    /// Prefira `create_upload_post` para uploads vindos do front.
    pub async fn create_upload_url(&self, key: String) -> Result<String, String> {
        let exp = PresigningConfig::expires_in(Duration::from_secs(60 * 60 * 24))
            .map_err(|_| "Não conseguimos definir a duração da url".to_string())?;

        let presigned = self
            .client
            .put_object()
            .bucket(self.bucket.to_string())
            .key(key)
            .presigned(exp)
            .await
            .map_err(|e| {
                format!(
                    "Não conseguimos gerar a uri do objeto requisitado: {}",
                    DisplayErrorContext(&e)
                )
            })?;

        Ok(presigned.uri().to_string())
    }

    pub async fn send(&self, key: String, body: ByteStream) -> Result<PutObjectOutput, String> {
        self.client
            .put_object()
            .bucket(self.bucket.to_string())
            .key(key)
            .body(body)
            .send()
            .await
            .map_err(|e| {
                format!(
                    "Não conseguimos gerar a uri do objeto requisitado: {}",
                    DisplayErrorContext(&e)
                )
            })
    }

    pub async fn get_object(&self, key: String) -> Result<String, String> {
        let req = self
            .client
            .get_object()
            .bucket(self.bucket.to_string())
            .key(key);

        let presigned = PresigningConfig::expires_in(Duration::from_secs(60 * 60 * 24))
            .map_err(|_| "Não conseguimos definir a duração da url".to_string())?;
        let presigned = req
            .presigned(presigned)
            .await
            .map_err(|e| {
                format!(
                    "Não conseguimos gerar a uri do objeto requisitado: {}",
                    DisplayErrorContext(&e)
                )
            })?;

        Ok(presigned.uri().to_string())
    }
}

#[cfg(test)]
mod aura_aws_s3_test {
    use crate::s3::{AwsS3Manager, PresignedPost};
    use aws_sdk_s3::primitives::ByteStream;
    use dotenv::dotenv;
    use image::{ImageBuffer, ImageFormat, Rgb};
    use reqwest::Client;
    use reqwest::multipart::{Form, Part};
    use std::env;
    use std::io::Cursor;

    async fn setup() -> AwsS3Manager {
        dotenv().ok();

        let bucket =
            env::var("AWS_S3_BUCKET_NAME").expect("AWS_S3_BUCKET_NAME not found in .env file");
        let region = env::var("AWS_REGION").expect("AWS_REGION not found in .env file");

        AwsS3Manager::new(bucket, region).await
    }

    fn generate_png_buffer(width: u32, height: u32) -> Vec<u8> {
        let img_buf = ImageBuffer::from_fn(width, height, |x, y| Rgb([x as u8, y as u8, 255]));

        let mut buffer = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img_buf)
            .write_to(&mut buffer, ImageFormat::Png)
            .expect("Erro ao escrever imagem no buffer");

        buffer.into_inner()
    }

    // simular o envio do conteúdo pelo front (presigned POST)
    // `content_type_field` permite sobrescrever o campo Content-Type para testar a policy
    async fn post_to_s3(
        post: PresignedPost,
        content: Vec<u8>,
        content_type_field: Option<&str>,
    ) -> reqwest::Response {
        let mut form = Form::new();
        for (k, v) in post.fields {
            let v = match (k.as_str(), content_type_field) {
                ("Content-Type", Some(over)) => over.to_string(),
                _ => v,
            };
            form = form.text(k, v);
        }
        // o arquivo precisa ser o último campo do form
        form = form.part(
            "file",
            Part::bytes(content)
                .file_name("test.png")
                .mime_str("image/png")
                .unwrap(),
        );

        Client::new()
            .post(post.url)
            .multipart(form)
            .send()
            .await
            .expect("Falha ao enviar requisição para o S3")
    }

    #[tokio::test]
    async fn upload_using_post() {
        let s3 = setup().await;

        let post = s3
            .create_upload_post("user_name-test.png".to_string(), "image/png", 1024 * 1024)
            .await
            .unwrap();

        let res = post_to_s3(post, generate_png_buffer(100, 100), None).await;
        let status = res.status();
        assert!(
            status.is_success(),
            "Erro no upload: {status} - {}",
            res.text().await.unwrap_or_default()
        );
    }

    #[tokio::test]
    async fn post_rejects_file_too_large() {
        let s3 = setup().await;

        // limite de 10 bytes: a imagem 100x100 passa muito disso
        let post = s3
            .create_upload_post("user_name-too-large.png".to_string(), "image/png", 10)
            .await
            .unwrap();

        let res = post_to_s3(post, generate_png_buffer(100, 100), None).await;
        assert!(
            res.status().is_client_error(),
            "Esperava 4xx, veio {}",
            res.status()
        );
    }

    #[tokio::test]
    async fn post_rejects_wrong_content_type() {
        let s3 = setup().await;

        let post = s3
            .create_upload_post("user_name-wrong-type.png".to_string(), "image/png", 1024 * 1024)
            .await
            .unwrap();

        let res = post_to_s3(post, generate_png_buffer(100, 100), Some("text/plain")).await;
        assert!(
            res.status().is_client_error(),
            "Esperava 4xx, veio {}",
            res.status()
        );
    }

    #[tokio::test]
    async fn upload_using_put_url() {
        let s3 = setup().await;

        let url = s3
            .create_upload_url("user_name-put-test.png".to_string())
            .await
            .unwrap();

        let res = Client::new()
            .put(url)
            .body(generate_png_buffer(100, 100))
            .header("Content-Type", "image/png")
            .send()
            .await
            .unwrap();

        let status = res.status();
        assert!(
            status.is_success(),
            "Erro no upload: {status} - {}",
            res.text().await.unwrap_or_default()
        );
    }

    #[tokio::test]
    async fn send_directly() {
        let s3 = setup().await;

        let file_content = "Hello, S3!".to_string().into_bytes();
        let body = ByteStream::from(file_content);

        s3.send("teste.txt".to_string(), body).await.unwrap();
    }
}