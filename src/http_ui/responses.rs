use bytes::Bytes;
use http_body_util::Full;
use hyper::{Response, StatusCode};
use serde::Serialize;

use super::templates;

pub fn json_response<T: Serialize>(status: StatusCode, data: &T) -> Response<Full<Bytes>> {
    let json = serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string());
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(json)))
        .unwrap()
}

pub fn html_response(status: StatusCode, html: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Full::new(Bytes::from(html)))
        .unwrap()
}

pub fn error_response(status: StatusCode, message: &str, wants_html: bool) -> Response<Full<Bytes>> {
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

pub fn not_found(wants_html: bool) -> Response<Full<Bytes>> {
    error_response(StatusCode::NOT_FOUND, "Not Found", wants_html)
}
