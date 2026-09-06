use axum::{
    body::Body,
    extract::Request,
    http::{
        HeaderMap, HeaderValue, Method, StatusCode,
        header::{
            ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, ETAG, IF_NONE_MATCH,
            VARY,
        },
    },
    response::Response,
};
use rust_embed::RustEmbed;

use crate::auth::{encode_hex, sha256};

#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
struct WebAssets;

const THEME_BOOTSTRAP_HASH: &str = include_str!("../../../web/dist/.csp-theme-hash");
const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";
const SHELL_CACHE: &str = "no-cache";

pub(crate) async fn serve(request: Request) -> Response {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }

    let requested = request.uri().path().trim_start_matches('/');
    if is_api_path(requested) || is_private_path(requested) {
        return empty(StatusCode::NOT_FOUND);
    }

    let path = if requested.is_empty() {
        "index.html"
    } else if WebAssets::get(requested).is_some() {
        requested
    } else if is_asset_path(requested) || looks_like_file(requested) {
        return empty(StatusCode::NOT_FOUND);
    } else {
        "index.html"
    };

    asset(path, request.method() == Method::HEAD, request.headers())
}

fn asset(path: &str, head_only: bool, request_headers: &HeaderMap) -> Response {
    let immutable = is_asset_path(path);
    let (representation, encoding) = if immutable {
        encoded_path(path, request_headers)
    } else {
        (path.to_owned(), None)
    };
    let Some(file) = WebAssets::get(&representation) else {
        return empty(StatusCode::NOT_FOUND);
    };

    let etag = (!immutable).then(|| format!("\"{}\"", encode_hex(&sha256(file.data.as_ref()))));
    if etag
        .as_deref()
        .is_some_and(|current| if_none_match(request_headers, current))
    {
        let mut response = empty(StatusCode::NOT_MODIFIED);
        set_response_headers(&mut response, path, encoding, etag.as_deref());
        return response;
    }

    let body = if head_only {
        Body::empty()
    } else {
        Body::from(file.data.into_owned())
    };
    let mut response = Response::new(body);
    set_response_headers(&mut response, path, encoding, etag.as_deref());
    response
}

fn set_response_headers(
    response: &mut Response,
    path: &str,
    encoding: Option<&'static str>,
    etag: Option<&str>,
) {
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type(path)));
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static(if is_asset_path(path) {
            IMMUTABLE_CACHE
        } else {
            SHELL_CACHE
        }),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );

    if let Some(etag) = etag {
        headers.insert(
            ETAG,
            HeaderValue::from_str(etag).expect("SHA-256 ETag is a valid header value"),
        );
    }
    if let Some(encoding) = encoding {
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static(encoding));
    }
    if is_asset_path(path) && compressed_variant_exists(path) {
        headers.insert(VARY, HeaderValue::from_static("Accept-Encoding"));
    }
    if path == "index.html" {
        let policy = format!(
            "default-src 'self'; script-src 'self' 'sha256-{}'; style-src 'self'; img-src 'self' data:; font-src 'none'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
            THEME_BOOTSTRAP_HASH.trim()
        );
        headers.insert(
            "content-security-policy",
            HeaderValue::from_str(&policy)
                .expect("build-generated CSP hash is a valid header value"),
        );
    }
}

fn encoded_path(path: &str, headers: &HeaderMap) -> (String, Option<&'static str>) {
    if accepts_encoding(headers, "br") && WebAssets::get(&format!("{path}.br")).is_some() {
        return (format!("{path}.br"), Some("br"));
    }
    if accepts_encoding(headers, "gzip") && WebAssets::get(&format!("{path}.gz")).is_some() {
        return (format!("{path}.gz"), Some("gzip"));
    }
    (path.to_owned(), None)
}

fn accepts_encoding(headers: &HeaderMap, expected: &str) -> bool {
    headers.get_all(ACCEPT_ENCODING).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value.split(',').any(|entry| {
                let mut parts = entry.trim().split(';');
                let name = parts.next().unwrap_or_default().trim();
                let disabled = parts.any(|parameter| {
                    parameter
                        .trim()
                        .strip_prefix("q=")
                        .and_then(|quality| quality.trim().parse::<f32>().ok())
                        .is_some_and(|quality| quality == 0.0)
                });
                !disabled && name.eq_ignore_ascii_case(expected)
            })
        })
    })
}

fn compressed_variant_exists(path: &str) -> bool {
    WebAssets::get(&format!("{path}.br")).is_some()
        || WebAssets::get(&format!("{path}.gz")).is_some()
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

fn is_api_path(path: &str) -> bool {
    path == "api" || path.starts_with("api/")
}

fn is_asset_path(path: &str) -> bool {
    path == "assets" || path.starts_with("assets/")
}

fn is_private_path(path: &str) -> bool {
    path.ends_with(".br")
        || path.ends_with(".gz")
        || path.contains('\\')
        || path
            .split('/')
            .any(|segment| segment == "." || segment == ".." || segment.starts_with('.'))
}

fn looks_like_file(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|name| name.contains('.'))
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn empty(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests {
    use axum::{
        body::to_bytes,
        http::{HeaderValue, Request, header},
    };

    use super::*;

    fn request(path: &str) -> Request<Body> {
        Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("test request")
    }

    async fn body(response: Response) -> Vec<u8> {
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body")
            .to_vec()
    }

    fn asset_path(extension: &str) -> String {
        WebAssets::iter()
            .map(|path| path.into_owned())
            .find(|path| path.starts_with("assets/") && path.ends_with(extension))
            .expect("built asset")
    }

    #[tokio::test]
    async fn index_and_spa_routes_serve_the_shell_but_api_and_missing_assets_do_not() {
        let index = serve(request("/")).await;
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(index.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
        assert!(
            String::from_utf8(body(index).await)
                .unwrap()
                .contains("<div id=\"root\"></div>")
        );

        let spa = serve(request("/nodes/0123456789abcdef0123456789abcdef")).await;
        assert_eq!(spa.status(), StatusCode::OK);
        assert!(
            String::from_utf8(body(spa).await)
                .unwrap()
                .contains("<!doctype html>")
        );

        let missing = serve(request("/assets/missing.js")).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert!(body(missing).await.is_empty());

        let api = serve(request("/api/not-real")).await;
        assert_eq!(api.status(), StatusCode::NOT_FOUND);
        assert!(
            !String::from_utf8(body(api).await)
                .unwrap()
                .contains("<!doctype html>")
        );
    }

    #[tokio::test]
    async fn built_assets_have_correct_types_cache_policy_and_encoding_negotiation() {
        let js = asset_path(".js");
        let css = asset_path(".css");

        let mut brotli_request = request(&format!("/{js}"));
        brotli_request
            .headers_mut()
            .insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, br"));
        let brotli = serve(brotli_request).await;
        assert_eq!(
            brotli.headers()[CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        assert_eq!(brotli.headers()[CACHE_CONTROL], IMMUTABLE_CACHE);
        assert_eq!(brotli.headers()[CONTENT_ENCODING], "br");
        assert_eq!(brotli.headers()[VARY], "Accept-Encoding");

        let mut gzip_request = request(&format!("/{css}"));
        gzip_request
            .headers_mut()
            .insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        let gzip = serve(gzip_request).await;
        assert_eq!(gzip.headers()[CONTENT_TYPE], "text/css; charset=utf-8");
        assert_eq!(gzip.headers()[CONTENT_ENCODING], "gzip");
        assert_eq!(gzip.headers()[VARY], "Accept-Encoding");

        let identity = serve(request(&format!("/{js}"))).await;
        assert!(identity.headers().get(CONTENT_ENCODING).is_none());
        assert_eq!(
            body(identity).await,
            WebAssets::get(&js).unwrap().data.as_ref()
        );
    }

    #[tokio::test]
    async fn shell_revalidates_and_carries_the_exact_bootstrap_csp_hash() {
        let first = serve(request("/")).await;
        assert_eq!(first.headers()[CACHE_CONTROL], SHELL_CACHE);
        let etag = first.headers()[ETAG].clone();
        let csp = first
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(!csp.contains("unsafe-inline"));
        assert!(!csp.contains("unsafe-eval"));
        assert!(csp.contains(&format!("'sha256-{}'", THEME_BOOTSTRAP_HASH.trim())));
        assert_eq!(first.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            first.headers()["referrer-policy"],
            "strict-origin-when-cross-origin"
        );

        let mut cached = request("/");
        cached.headers_mut().insert(IF_NONE_MATCH, etag.clone());
        let cached = serve(cached).await;
        assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(cached.headers()[ETAG], etag);
        assert_eq!(cached.headers()[CACHE_CONTROL], SHELL_CACHE);
        assert!(body(cached).await.is_empty());

        let mut stale = request("/");
        stale
            .headers_mut()
            .insert(IF_NONE_MATCH, HeaderValue::from_static("\"stale\""));
        assert_eq!(serve(stale).await.status(), StatusCode::OK);
    }

    #[test]
    fn content_types_and_encoding_parser_cover_the_static_contract() {
        assert_eq!(content_type("x.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("x.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("x.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("x.json"), "application/json");
        assert_eq!(content_type("x.svg"), "image/svg+xml");
        assert_eq!(content_type("x.png"), "image/png");
        assert_eq!(content_type("x.ico"), "image/x-icon");
        assert_eq!(content_type("x.webp"), "image/webp");
        assert_eq!(content_type("x.wasm"), "application/wasm");
        assert_eq!(content_type("x.bin"), "application/octet-stream");

        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip;q=0.0, Br;q=1"),
        );
        assert!(accepts_encoding(&headers, "br"));
        assert!(!accepts_encoding(&headers, "gzip"));
    }

    #[tokio::test]
    async fn precompressed_and_build_metadata_files_are_not_public_paths() {
        let js = asset_path(".js");
        assert_eq!(
            serve(request(&format!("/{js}.br"))).await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve(request("/.csp-theme-hash")).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}
