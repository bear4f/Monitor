use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH},
    },
    response::Response,
};

use crate::app::AppState;

pub(crate) async fn snapshot(State(state): State<AppState>, request: Request) -> Response {
    let snapshot = state.public_snapshot.load().await;
    if if_none_match(
        request.headers(),
        snapshot.etag.to_str().unwrap_or_default(),
    ) {
        return response(StatusCode::NOT_MODIFIED, &snapshot.etag, Body::empty());
    }

    let body = Body::from(Bytes::from_owner(snapshot.body.clone()));
    let mut response = response(StatusCode::OK, &snapshot.etag, body);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn response(status: StatusCode, etag: &HeaderValue, body: Body) -> Response {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().insert(ETAG, etag.clone());
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

fn if_none_match(headers: &HeaderMap, current: &str) -> bool {
    headers.get_all(IF_NONE_MATCH).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|candidate| candidate.trim() == current)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn if_none_match_accepts_exact_strong_tag_in_a_list() {
        let mut headers = HeaderMap::new();
        headers.insert(
            IF_NONE_MATCH,
            HeaderValue::from_static("\"old\", \"current\""),
        );
        assert!(if_none_match(&headers, "\"current\""));
        assert!(!if_none_match(&headers, "\"other\""));
        assert!(!if_none_match(&headers, "W/\"old\""));
    }
}
