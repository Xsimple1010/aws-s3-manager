use crate::common_types::UserIdentity;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct S3Entity {
    #[serde(rename = "s3SchemaVersion")]
    pub s3_schema_version: String,

    #[serde(rename = "configurationId")]
    pub configuration_id: String,

    pub bucket: Bucket,
    pub object: S3Object,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Bucket {
    pub name: String,

    #[serde(rename = "ownerIdentity")]
    pub owner_identity: UserIdentity,

    pub arn: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct S3Object {
    pub key: String,
    pub size: u64,

    #[serde(rename = "eTag")]
    pub e_tag: String,

    pub sequencer: String,
}