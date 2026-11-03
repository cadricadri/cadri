use axum::http::StatusCode;

use crate::memory::s3::S3Fail;
use crate::ApiError;

#[derive(Debug)]
pub(super) enum Fail {
    Conflict,
    Api(ApiError),
}

impl Fail {
    pub(super) fn api(status: StatusCode, message: impl Into<String>) -> Self {
        Fail::Api(ApiError::new(status, message))
    }

    pub(super) fn git(detail: impl AsRef<str>) -> Self {
        let detail = detail.as_ref();
        eprintln!("memory git: {detail}");
        Fail::Api(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("memory git failed: {detail}"),
        ))
    }
}

pub(super) fn s3_fail(err: S3Fail) -> Fail {
    match err {
        S3Fail::Conflict => Fail::Conflict,
        S3Fail::Message(message) => Fail::git(message),
    }
}

pub(super) fn into_api(err: Fail) -> ApiError {
    match err {
        Fail::Conflict => ApiError::new(StatusCode::CONFLICT, "memory book changed"),
        Fail::Api(err) => err,
    }
}
