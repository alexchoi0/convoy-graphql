use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("Failed to build GraphQL schema: {message}")]
    BuildError { message: String },

    #[error("Failed to parse: {message}")]
    ParseError { message: String },

    #[error("N+1 query detected: Type '{type_name}' is used in a list context but resolver(s) '{resolver}' are not batched. Add #[batch(key = \"...\", delay_ms = ...)] to fix.")]
    NPlusOne { type_name: String, resolver: String },
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Schema(#[from] SchemaError),
}

pub type Result<T> = std::result::Result<T, Error>;
