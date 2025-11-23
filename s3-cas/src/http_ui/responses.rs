use bytes::Bytes;
use http_body_util::{Full, BodyExt};
use hyper::{Response, StatusCode};
use serde::Serialize;

use super::templates;
use super::HttpBody;

pub fn map_response(response: Response<Full<Bytes>>) -> Response<HttpBody> {
    let (parts, body) = response.into_parts();
    let body = body.map_err(|_| -> Box<dyn std::error::Error + Send + Sync> { unreachable!() }).boxed();
    Response::from_parts(parts, body)
}

pub fn json_response<T: Serialize>(status: StatusCode, data: &T) -> Response<HttpBody> {
    let json = serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string());
    let resp = Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(json)))
        .unwrap();
    map_response(resp)
}

pub fn html_response(status: StatusCode, html: String) -> Response<HttpBody> {
    let resp = Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Full::new(Bytes::from(html)))
        .unwrap();
    map_response(resp)
}

pub fn error_response(status: StatusCode, message: &str, wants_html: bool) -> Response<HttpBody> {
    if wants_html {
        html_response(status, templates::error_page(message))
    } else {
        let error = serde_json::json!({
            "error": message,
            "status": status.as_u16()
        });
        json_response(status, &error)
    }
}

pub fn not_found(wants_html: bool) -> Response<HttpBody> {
    error_response(StatusCode::NOT_FOUND, "Not Found", wants_html)
}
