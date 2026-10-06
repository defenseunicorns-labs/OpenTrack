use thiserror::Error;

#[derive(Debug, Error)]
pub enum SapientError {
    #[error("invalid SAPIENT protobuf: {0}")]
    Decode(String),
    #[error("descriptor pool error: {0}")]
    Descriptor(String),
}
