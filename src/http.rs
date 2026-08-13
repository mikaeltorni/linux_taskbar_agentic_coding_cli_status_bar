//! HTTP opener abstraction so collectors stay testable without the network.

use std::collections::HashMap;
use std::time::Duration;

/// Outgoing request used by collectors.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

/// Response or HTTP-status error from an opener.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: HashMap<String, String>,
}

impl HttpResponse {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Failure from the transport itself (not an HTTP 4xx/5xx body).
#[derive(Debug, Clone)]
pub struct HttpError {
    pub message: String,
    pub status: Option<u16>,
    pub body: Vec<u8>,
    pub headers: HashMap<String, String>,
}

impl HttpError {
    pub fn transport(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
            body: Vec::new(),
            headers: HashMap::new(),
        }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// `urlopen`-compatible callable used by collectors.
pub trait HttpOpener: Send + Sync {
    fn open(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError>;
}

/// Production opener using `ureq`.
pub struct UreqOpener;

impl HttpOpener for UreqOpener {
    fn open(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let mut builder = ureq::request(&request.method, &request.url);
        builder = builder.timeout(request.timeout);
        for (name, value) in &request.headers {
            builder = builder.set(name, value);
        }
        let result = match &request.body {
            Some(body) => builder.send_bytes(body),
            None => builder.call(),
        };
        match result {
            Ok(response) => read_ureq_response(response),
            Err(ureq::Error::Status(code, response)) => {
                let parsed = read_ureq_response(response).unwrap_or_else(|err| HttpResponse {
                    status: code,
                    body: err.body,
                    headers: err.headers,
                });
                Err(HttpError {
                    message: format!("HTTP {code}"),
                    status: Some(code),
                    body: parsed.body,
                    headers: parsed.headers,
                })
            }
            Err(exc) => Err(HttpError::transport(exc.to_string())),
        }
    }
}

fn read_ureq_response(response: ureq::Response) -> Result<HttpResponse, HttpError> {
    let status = response.status();
    let mut headers = HashMap::new();
    for name in ["Retry-After", "retry-after", "Content-Type", "content-type"] {
        if let Some(value) = response.header(name) {
            headers.insert(name.to_string(), value.to_string());
        }
    }
    let mut body = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut body)
        .map_err(|exc| HttpError::transport(exc.to_string()))?;
    Ok(HttpResponse {
        status,
        body,
        headers,
    })
}

/// Header lookup that matches Python's case-insensitive `Retry-After`.
pub fn header_ci(headers: &HashMap<String, String>, name: &str) -> Option<String> {
    headers.iter().find_map(|(key, value)| {
        if key.eq_ignore_ascii_case(name) {
            Some(value.clone())
        } else {
            None
        }
    })
}
