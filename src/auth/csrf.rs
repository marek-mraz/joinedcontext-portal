//! Double-submit CSRF protection for every mutating Portal API call.
//!
//! `SameSite=Lax` already keeps the session cookie off cross-site POSTs; this is the second
//! lock: the UI reads the readable `jc_csrf` cookie and echoes it in `X-CSRF-Token`.

use axum::extract::Request;
use axum::http::{HeaderMap, Method};
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use openidconnect::CsrfToken;

/// Readable by the UI on purpose: the double-submit token is not a secret to the page.
pub const CSRF_COOKIE: &str = "jc_csrf";
pub const CSRF_HEADER: &str = "x-csrf-token";
const CSRF_TTL_SECS: i64 = 12 * 60 * 60;

/// Fresh token, minted with the same CSPRNG the OIDC state uses.
pub fn new_token() -> String {
    CsrfToken::new_random().secret().clone()
}

/// The cookie the UI reads; `HttpOnly` is deliberately off, `Secure` and `Lax` are not.
pub fn cookie(token: String) -> Cookie<'static> {
    Cookie::build((CSRF_COOKIE, token))
        .path("/")
        .http_only(false)
        .secure(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(CSRF_TTL_SECS))
        .build()
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// True when the request may proceed: safe methods always, mutations only with a matching token.
pub fn is_allowed(method: &Method, headers: &HeaderMap) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    let jar = CookieJar::from_headers(headers);
    let Some(expected) = jar.get(CSRF_COOKIE).map(|c| c.value().to_string()) else {
        return false;
    };
    let Some(presented) = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    !expected.is_empty() && constant_time_eq(&expected, presented)
}

/// Middleware for the `/api/v1` router. Rejects with problem+json, never with a bare 403 body.
pub async fn require_csrf(
    request: Request,
    next: Next,
) -> Result<Response, crate::error::ApiError> {
    if is_allowed(request.method(), request.headers()) {
        Ok(next.run(request).await)
    } else {
        Err(crate::error::ApiError::Forbidden)
    }
}

/// Adds a CSRF cookie to a jar, returning the token that was issued.
pub fn issue(jar: CookieJar) -> (CookieJar, String) {
    let token = new_token();
    (jar.add(cookie(token.clone())), token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(cookie_value: Option<&str>, header_value: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(v) = cookie_value {
            headers.insert(
                axum::http::header::COOKIE,
                format!("{CSRF_COOKIE}={v}").parse().expect("cookie header"),
            );
        }
        if let Some(v) = header_value {
            headers.insert(CSRF_HEADER, v.parse().expect("csrf header"));
        }
        headers
    }

    #[test]
    fn safe_methods_need_no_token() {
        assert!(is_allowed(&Method::GET, &headers_with(None, None)));
        assert!(is_allowed(&Method::HEAD, &headers_with(None, None)));
    }

    #[test]
    fn mutation_without_token_is_refused() {
        assert!(!is_allowed(&Method::POST, &headers_with(None, None)));
        assert!(!is_allowed(
            &Method::DELETE,
            &headers_with(Some("abc"), None)
        ));
        assert!(!is_allowed(
            &Method::PATCH,
            &headers_with(None, Some("abc"))
        ));
    }

    #[test]
    fn mutation_with_mismatched_token_is_refused() {
        assert!(!is_allowed(
            &Method::POST,
            &headers_with(Some("abc"), Some("abd"))
        ));
        assert!(!is_allowed(
            &Method::POST,
            &headers_with(Some("abc"), Some("abcd"))
        ));
        assert!(!is_allowed(
            &Method::POST,
            &headers_with(Some(""), Some(""))
        ));
    }

    #[test]
    fn mutation_with_matching_token_passes() {
        let token = new_token();
        assert!(is_allowed(
            &Method::POST,
            &headers_with(Some(&token), Some(&token))
        ));
    }

    #[test]
    fn cookie_is_readable_by_the_ui_but_still_secure() {
        let c = cookie("t".into());
        assert_eq!(c.http_only(), Some(false));
        assert_eq!(c.secure(), Some(true));
        assert_eq!(c.same_site(), Some(SameSite::Lax));
    }
}
