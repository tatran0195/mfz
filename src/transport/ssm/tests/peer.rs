//! Test-only bounded HTTP peer for the REAL AWS SDK serializers/signers. No
//! service implementation or credential provider is substituted in production.
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
    time::timeout,
};

pub(crate) struct Response {
    content_type: &'static str,
    body: Vec<u8>,
}
impl Response {
    pub(crate) fn new(content_type: &'static str, body: Vec<u8>) -> Self {
        Self { content_type, body }
    }
}

pub(crate) struct Request {
    pub headers: String,
    pub body: Vec<u8>,
}

pub(crate) struct HttpPeer {
    pub url: String,
    task: Option<JoinHandle<Vec<Request>>>,
}
impl HttpPeer {
    pub(crate) async fn new(responses: Vec<Response>) -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut tcp, _) = timeout(Duration::from_secs(10), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                let mut bytes = Vec::new();
                let end = loop {
                    assert!(
                        bytes.len() < 16384,
                        "SDK request headers unexpectedly large"
                    );
                    assert_ne!(
                        timeout(Duration::from_secs(10), tcp.read_buf(&mut bytes))
                            .await
                            .unwrap()
                            .unwrap(),
                        0
                    );
                    if let Some(position) =
                        bytes.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        break position + 4;
                    }
                };
                let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                let len = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .expect("SDK must serialize an explicit content length");
                assert!(len < 65536, "SDK request body unexpectedly large");
                while bytes.len() < end + len {
                    assert_ne!(
                        timeout(Duration::from_secs(10), tcp.read_buf(&mut bytes))
                            .await
                            .unwrap()
                            .unwrap(),
                        0
                    );
                }
                requests.push(Request {
                    headers,
                    body: bytes[end..end + len].to_vec(),
                });
                let header = format!("HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nx-amzn-RequestId: local-test-request\r\n\r\n", response.content_type, response.body.len());
                tcp.write_all(header.as_bytes()).await.unwrap();
                tcp.write_all(&response.body).await.unwrap();
            }
            requests
        });
        Self {
            url,
            task: Some(task),
        }
    }
    pub(crate) async fn requests(mut self) -> Vec<Request> {
        let requests = timeout(Duration::from_secs(10), self.task.as_mut().unwrap())
            .await
            .unwrap()
            .unwrap();
        self.task.take();
        requests
    }
}
impl Drop for HttpPeer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
