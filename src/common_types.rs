use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct UserIdentity {
    #[serde(rename = "principalId")]
    pub principal_id: String,
}