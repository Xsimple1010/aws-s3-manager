use aws_config::BehaviorVersion;
use aws_config::meta::credentials::CredentialsProviderChain;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct AwsS3Manager {
    bucket: Arc<String>,
    region: Arc<String>,
}

impl AwsS3Manager {
    pub fn new(bucket: String, region: String) -> Self {
        AwsS3Manager {
            bucket: Arc::new(bucket),
            region: Arc::new(region),
        }
    }

    pub async fn setup_client(&self) -> Client {
        let config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(self.region.to_string()))
            .credentials_provider(CredentialsProviderChain::default_provider().await)
            .load()
            .await;

        Client::new(&config)
    }

    pub async fn create_upload_url(&self, key: String) -> Result<String, String> {
        let client = self.setup_client().await;

        let exp = PresigningConfig::expires_in(Duration::from_secs(60 * 60 * 24))
            .map_err(|_| "Não conseguimos definir a duração da url".to_string())?;

        // Gerar presigned URL para PutObject
        let presigned = client
            .put_object()
            .bucket(self.bucket.to_string())
            .key(key)
            .presigned(exp)
            .await
            .map_err(|_| "Não conseguimos gerar a uri do objeto requisitado".to_string())?;

        Ok(presigned.uri().to_string())
    }

    pub async fn send(&self, key: String, body: ByteStream) {
        self.setup_client()
            .await
            .put_object()
            .bucket(self.bucket.to_string())
            .key(key)
            .body(body)
            .send()
            .await
            .unwrap();
    }

    pub async fn get_object(&self, key: String) -> Result<String, String> {
        let req = self
            .setup_client()
            .await
            .get_object()
            .bucket(self.bucket.to_string())
            .key(key);

        let presigned = PresigningConfig::expires_in(Duration::from_secs(60 * 60 * 24))
            .map_err(|_| "Não conseguimos definir a duração da url".to_string())?;
        let presigned = req
            .presigned(presigned)
            .await
            .map_err(|_| "Não conseguimos gerar a uri do objeto requisitado".to_string())?;

        Ok(presigned.uri().to_string())
    }
}

#[cfg(test)]
mod aura_aws_s3_test {
    use crate::s3::AwsS3Manager;
    use aws_sdk_s3::primitives::ByteStream;
    use dotenv::dotenv;
    use image::{ImageBuffer, ImageFormat, Rgb};
    use reqwest;
    use reqwest::Client;
    use std::env;
    use std::io::Cursor;

    fn setup() -> AwsS3Manager {
        dotenv().ok();

        let bucket =
            env::var("AWS_S3_BUCKET_NAME").expect("AWS_S3_BUCKET_NAME not found in .env file");
        let region = env::var("AWS_REGION").expect("AWS_REGION not found in .env file");

        AwsS3Manager::new(bucket, region)
    }

    fn generate_png_buffer(width: u32, height: u32) -> Vec<u8> {
        let img_buf = ImageBuffer::from_fn(width, height, |x, y| Rgb([x as u8, y as u8, 255]));

        let mut buffer = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img_buf)
            .write_to(&mut buffer, ImageFormat::Png)
            .expect("Erro ao escrever imagem no buffer");

        buffer.into_inner()
    }

    // simular o envio do conteúdo pelo front
    async fn send_to_s3(url: &str, content: Vec<u8>) -> Result<(), Box<dyn std::error::Error>> {
        let response = Client::new()
            .put(url)
            .body(content)
            .header("Content-Type", "image/png")
            .send()
            .await?;

        if response.status().is_success() {
            println!("✅ Upload bem-sucedido! Status: {}", response.status());
        } else {
            println!("❌ Erro no upload: {}", response.status());
            println!("Detalhes: {:?}", response.text().await?);
        }

        Ok(())
    }

    #[tokio::test]
    async fn upload_using_url() {
        let s3_manager = setup();

        let url = s3_manager
            .create_upload_url("test.png".to_string())
            .await
            .unwrap();

        let img = generate_png_buffer(100, 100);

        let result = send_to_s3(&url, img).await;

        if let Err(e) = result {
            panic!("Erro ao enviar para o S3: {}", e);
        }
    }

    #[tokio::test]
    async fn send_directly() {
        let s3_manager = setup();

        let file_content = "Hello, S3!".to_string().into_bytes();
        let body = ByteStream::from(file_content);

        s3_manager.send("teste.txt".to_string(), body).await;
    }
}
