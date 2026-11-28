//! TLS ingress for the delegated job facade. Only a typed facade message can
//! cross from this listener to the local daemon's owner socket.

use std::convert::Infallible;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use fabric_client::Client;
use fabric_client::facade::{Bearer, ClientMessage, MAX_ANSWER_BYTES, MAX_MESSAGE_BYTES};
use fabric_core::id::NodeId;
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{
    ALLOW, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HeaderValue, WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};

const ENDPOINT: &str = "/v1/facade";
const READ_TIME: Duration = Duration::from_secs(5);
const AUTHORITY_TIME: Duration = Duration::from_secs(30);
const CONNECTION_TIME: Duration = Duration::from_secs(50);

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// The only authority that receives facade calls through this listener.
    #[arg(long)]
    authority: NodeId,
    /// TLS listener address. Set a non-loopback address to accept remote clients.
    #[arg(long, default_value = "127.0.0.1:8443")]
    listen: SocketAddr,
    /// PEM certificate chain, with the server certificate first.
    #[arg(long)]
    cert: PathBuf,
    /// PEM private key matching the certificate.
    #[arg(long)]
    key: PathBuf,
}

pub(crate) async fn serve(client: Client, args: Args) -> Result<ExitCode> {
    let certs = CertificateDer::pem_file_iter(args.cert)
        .context("read the gateway certificate chain")?
        .collect::<Result<Vec<_>, _>>()
        .context("parse the gateway certificate chain")?;
    let key = PrivateKeyDer::from_pem_file(args.key).context("read the gateway private key")?;
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("configure the gateway certificate and key")?;
    let tls = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind(args.listen).await?;
    writeln!(
        std::io::stdout().lock(),
        "{{\"endpoint\":\"https://{}{ENDPOINT}\"}}",
        listener.local_addr()?
    )?;
    let slots = Arc::new(Semaphore::new(16));
    loop {
        let permit = slots.clone().acquire_owned().await?;
        let (stream, _) = listener.accept().await?;
        let tls = tls.clone();
        let client = client.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let Ok(Ok(stream)) = timeout(READ_TIME, tls.accept(stream)).await else {
                return;
            };
            let handler = service_fn(move |request| {
                let client = client.clone();
                async move {
                    let response = match dispatch(request, &client, args.authority).await {
                        Ok(bytes) => json_response(StatusCode::OK, bytes),
                        Err((status, reason)) => {
                            json_response(status, format!("{{\"error\":\"{reason}\"}}").into())
                        }
                    };
                    Ok::<_, Infallible>(response)
                }
            });
            let mut http = http1::Builder::new();
            http.keep_alive(false)
                .max_headers(32)
                .max_buf_size(16 * 1024)
                .timer(TokioTimer::new())
                .header_read_timeout(READ_TIME);
            let _ = timeout(
                CONNECTION_TIME,
                http.serve_connection(TokioIo::new(stream), handler),
            )
            .await;
        });
    }
}

type HttpError = (StatusCode, &'static str);

fn json_response(status: StatusCode, bytes: Bytes) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(bytes));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    match status {
        StatusCode::UNAUTHORIZED => {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        StatusCode::METHOD_NOT_ALLOWED => {
            response
                .headers_mut()
                .insert(ALLOW, HeaderValue::from_static("POST"));
        }
        _ => {}
    }
    response
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let (scheme, token) = values.next()?.to_str().ok()?.split_once(' ')?;
    (values.next().is_none()
        && scheme.eq_ignore_ascii_case("bearer")
        && Bearer::parse_hex(token).is_ok())
    .then_some(token)
}

async fn dispatch(
    request: Request<Incoming>,
    client: &Client,
    authority: NodeId,
) -> Result<Bytes, HttpError> {
    if request
        .uri()
        .path_and_query()
        .map(hyper::http::uri::PathAndQuery::as_str)
        != Some(ENDPOINT)
    {
        return Err((StatusCode::NOT_FOUND, "not_found"));
    }
    if request.method() != Method::POST {
        return Err((StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed"));
    }
    let token = bearer(request.headers())
        .ok_or((StatusCode::UNAUTHORIZED, "invalid_bearer"))?
        .to_owned();
    let media = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim);
    if !media.is_some_and(|value| value.eq_ignore_ascii_case("application/json")) {
        return Err((StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type"));
    }
    let body = timeout(
        READ_TIME,
        Limited::new(request.into_body(), MAX_MESSAGE_BYTES).collect(),
    )
    .await
    .map_err(|_| (StatusCode::REQUEST_TIMEOUT, "request_timeout"))?
    .map_err(|error| {
        if error.downcast_ref::<LengthLimitError>().is_some() {
            (StatusCode::PAYLOAD_TOO_LARGE, "request_too_large")
        } else {
            (StatusCode::BAD_REQUEST, "invalid_body")
        }
    })?
    .to_bytes();
    let message = parse_message(&body, &token)?;
    let answer = timeout(
        AUTHORITY_TIME,
        client.facade_call(authority, &message, |_| {}),
    )
    .await
    .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, "authority_timeout"))?
    .map_err(|_| (StatusCode::BAD_GATEWAY, "authority_unavailable"))?;
    let bytes = serde_json::to_vec(&answer.to_json())
        .map_err(|_| (StatusCode::BAD_GATEWAY, "invalid_answer"))?;
    if bytes.len() > MAX_ANSWER_BYTES {
        return Err((StatusCode::BAD_GATEWAY, "answer_too_large"));
    }
    Ok(bytes.into())
}

fn parse_message(body: &[u8], token: &str) -> Result<ClientMessage, HttpError> {
    let invalid = (StatusCode::BAD_REQUEST, "invalid_message");
    let mut json: Value = serde_json::from_slice(body).map_err(|_| invalid)?;
    let obj = json.as_object_mut().ok_or(invalid)?;
    if obj.contains_key("token") || obj.contains_key("authority") {
        return Err(invalid);
    }
    obj.insert("token".into(), Value::String(token.into()));
    if serde_json::to_vec(&json).map_err(|_| invalid)?.len() > MAX_MESSAGE_BYTES {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "request_too_large"));
    }
    ClientMessage::from_json(&json).map_err(|_| invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_one_header_and_refuses_ambiguous_or_malformed_bearers() {
        let token = "ab".repeat(32);
        let mut headers = HeaderMap::new();
        assert_eq!(bearer(&headers), None);
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer invalid"));
        assert_eq!(bearer(&headers), None);
        headers.insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        assert_eq!(bearer(&headers), Some(token.as_str()));
        headers.append(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        assert_eq!(bearer(&headers), None);
    }

    #[test]
    fn only_a_typed_facade_request_with_header_credentials_can_reach_the_socket() {
        let token = "ab".repeat(32);
        let json = json!({"facade":1,"type":"inspect","job":"01JABCDEFGHJKMNPQRSTVWXYZ0"});
        let encode = |v: &Value| serde_json::to_vec(v).unwrap();
        assert!(matches!(
            parse_message(&encode(&json), &token),
            Ok(ClientMessage::Inspect { .. })
        ));
        for extra in ["token", "authority"] {
            let mut value = json.clone();
            value[extra] = json!("untrusted");
            assert!(parse_message(&encode(&value), &token).is_err());
        }
        let owner = json!({"type":"facade_mint","facade":1,"actor":"operator"});
        assert!(parse_message(&encode(&owner), &token).is_err());
    }
}
