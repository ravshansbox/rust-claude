use super::*;

#[derive(Clone)]
pub(super) struct HttpRequest {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) headers: HashMap<String, String>,
    pub(super) body: Value,
    raw_body: String,
}

impl HttpRequest {
    pub(super) fn form(&self) -> HashMap<String, String> {
        reqwest::Url::parse(&format!("http://form/?{}", self.raw_body))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect()
    }
}

pub(super) struct HttpReply {
    pub(super) status: u16,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: String,
    pub(super) delay: Duration,
}

impl HttpReply {
    pub(super) fn json(body: &Value) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }

    pub(super) fn events(events: &[Value]) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            body: events
                .iter()
                .map(|event| format!("event: message\ndata: {event}\n\n"))
                .collect(),
            delay: Duration::ZERO,
        }
    }

    pub(super) fn status(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    pub(super) fn stall() -> Self {
        Self::status(0, "")
    }

    pub(super) fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.retain(|(existing, _)| existing != name);
        self.headers.push((name.into(), value.into()));
        self
    }

    pub(super) fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

pub(super) type Requests = Arc<Mutex<Vec<HttpRequest>>>;

/// A local HTTP server that answers each request with `handler` and
/// records the requests it received.
pub(super) async fn http_server(
    handler: impl Fn(&HttpRequest) -> HttpReply + Send + Sync + 'static,
) -> (String, Requests) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let handler = handler.clone();
            let recorded = recorded.clone();
            tokio::spawn(async move {
                let Some(request) = read_http_request(&mut stream).await else {
                    return;
                };
                recorded.lock().unwrap().push(request.clone());
                let reply = handler(&request);
                if reply.status == 0 {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    return;
                }
                tokio::time::sleep(reply.delay).await;
                let mut response = format!(
                    "HTTP/1.1 {} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                    reply.status,
                    reply.body.len()
                );
                for (name, value) in reply.headers {
                    let _ = write!(response, "{name}: {value}\r\n");
                }
                response.push_str("\r\n");
                response.push_str(&reply.body);
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (url, requests)
}

pub(super) async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Option<HttpRequest> {
    let mut data = Vec::new();
    let header_end = loop {
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        read_chunk(stream, &mut data).await?;
    };
    let head = String::from_utf8_lossy(&data[..header_end]);
    let mut lines = head.lines();
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    while data.len() < header_end + length {
        read_chunk(stream, &mut data).await?;
    }
    let body = serde_json::from_slice(&data[header_end..]).unwrap_or(Value::Null);
    Some(HttpRequest {
        method,
        path,
        headers,
        body,
        raw_body: String::from_utf8_lossy(&data[header_end..]).into_owned(),
    })
}
