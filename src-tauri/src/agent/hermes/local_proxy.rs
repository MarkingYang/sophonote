//! MODEL-11④：loopback 剥鉴权代理。
//!
//! Hermes 要求每个供应商都有「可用凭据」并把其原样作为 `Authorization: Bearer`
//! 发出（LM Studio 等本地服务器默认忽略该头）。Ollama 等端点则会校验请求携带的
//! 鉴权头——占位 Bearer 一发即 401，而不带该头的请求畅通。SophoNote 因此在
//! loopback 上运行一个极薄代理：config.yaml 中托管条目的 `base_url` 指向
//! `/mbp/{实例id}` 前缀的代理地址，代理剥离 `Authorization` 后转发到用户设置的
//! 真实端点。Hermes 目录可见性（需要非空凭据）与端点免鉴权特性由此同时成立。
//!
//! 安全边界：仅绑定 127.0.0.1；只转发到设置中登记过的 http 目标（非开放代理，
//! 实例 id 必须命中注册表）；不记录请求体与任何凭据。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{OnceLock, RwLock};
use std::thread;
use std::time::Duration;

const PATH_PREFIX: &str = "/mbp/";
const MAX_HEAD_BYTES: usize = 64 * 1024;

static TARGETS: OnceLock<RwLock<HashMap<String, ProxyTarget>>> = OnceLock::new();
static PROXY_PORT: OnceLock<u16> = OnceLock::new();

#[derive(Debug, Clone, PartialEq)]
struct ProxyTarget {
    host: String,
    port: u16,
    /// 真实 base_url 的路径前缀（如 `/v1`），无尾斜杠。
    path_prefix: String,
}

fn targets_slot() -> &'static RwLock<HashMap<String, ProxyTarget>> {
    TARGETS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 是否为可走代理的明文 http 端点（本地/内网免鉴权端点的常态）。
/// https 目标返回 false，调用方回退为直连。
pub fn is_http_target(base_url: &str) -> bool {
    parse_http_target(base_url).is_some()
}

/// 是否为本地/私有网络端点（loopback、RFC1918、.local）。免鉴权语义只对这类
/// 端点成立；云上 https 端点被标记免鉴权通常来自抽屉误切换。
pub fn is_local_endpoint(base_url: &str) -> bool {
    let Some(target) = parse_http_target(base_url) else {
        return false;
    };
    let host = target.host.to_ascii_lowercase();
    if host == "localhost" || host == "::1" || host.ends_with(".local") {
        return true;
    }
    if host.starts_with("127.") || host.starts_with("10.") || host.starts_with("192.168.") {
        return true;
    }
    if let Some(second) = host.strip_prefix("172.") {
        if let Some(first) = second.split('.').next() {
            if let Ok(octet) = first.parse::<u16>() {
                return (16..=31).contains(&octet);
            }
        }
    }
    false
}

fn parse_http_target(base_url: &str) -> Option<ProxyTarget> {
    let rest = base_url.trim().strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok()?),
        None => (authority, 80u16),
    };
    if host.is_empty() {
        return None;
    }
    let path_prefix = path.trim_end_matches('/').to_string();
    Some(ProxyTarget {
        host: host.to_string(),
        port,
        path_prefix,
    })
}

/// 登记 实例 id → 真实端点 映射；返回可走代理的实例（http 目标）。
pub fn set_proxy_targets(entries: &[(String, String)]) -> Vec<String> {
    let mut proxied = Vec::new();
    let mut map = HashMap::new();
    for (id, base_url) in entries {
        if let Some(target) = parse_http_target(base_url) {
            map.insert(id.clone(), target);
            proxied.push(id.clone());
        }
    }
    if let Ok(mut slot) = targets_slot().write() {
        *slot = map;
    }
    proxied
}

/// 代理基址：Hermes OpenAI SDK 会在其后追加 `/chat/completions` 等路径。
pub fn proxy_base_url(port: u16, instance_id: &str) -> String {
    format!("http://127.0.0.1:{port}{PATH_PREFIX}{instance_id}")
}

/// 已启动代理的 loopback 端口；未启动返回 None。
pub fn proxy_port() -> Option<u16> {
    PROXY_PORT.get().copied()
}

/// 启动代理（进程内单例，幂等），返回 loopback 端口。
pub fn ensure_proxy() -> Result<u16, String> {
    if let Some(port) = PROXY_PORT.get() {
        return Ok(*port);
    }
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind local no-auth proxy: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local proxy addr: {e}"))?
        .port();
    let _ = PROXY_PORT.set(port);
    thread::Builder::new()
        .name("sophonote-local-proxy".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_nodelay(true);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(600)));
                thread::spawn(move || {
                    if let Err(error) = handle_connection(&mut stream) {
                        eprintln!("[local-proxy] {error}");
                    }
                });
            }
        })
        .map_err(|e| format!("spawn local proxy thread: {e}"))?;
    eprintln!("[hermes] local no-auth proxy listening on 127.0.0.1:{port}");
    Ok(port)
}

fn respond_error(stream: &mut TcpStream, status: &str, message: &str) {
    let body = format!("{{\"error\":\"{message}\"}}");
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// 读取请求头（至 `\r\n\r\n`），返回 (头文本, 已预读但超出头部的字节)。
fn read_request_head(stream: &mut TcpStream) -> Result<(String, Vec<u8>), String> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if buf.len() > MAX_HEAD_BYTES {
            return Err("request head too large".into());
        }
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            let head_end = pos + 4;
            let extra = buf[head_end..].to_vec();
            buf.truncate(head_end);
            return Ok((String::from_utf8_lossy(&buf).into_owned(), extra));
        }
        let n = stream
            .read(&mut chunk)
            .map_err(|e| format!("read request head: {e}"))?;
        if n == 0 {
            return Err("client closed before head complete".into());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn handle_connection(stream: &mut TcpStream) -> Result<(), String> {
    let (head, extra) = read_request_head(stream)?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or("empty request")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("bad request line")?.to_string();
    let raw_path = parts.next().ok_or("bad request line")?.to_string();

    let tail = raw_path
        .strip_prefix(PATH_PREFIX)
        .ok_or_else(|| format!("not a proxy path: {raw_path}"))?;
    let (instance_id, rest_with_query) = match tail.split_once('/') {
        Some((id, rest)) => (id, format!("/{rest}")),
        None => (tail, "/".to_string()),
    };

    let target = {
        let slot = targets_slot().read().map_err(|_| "targets lock")?;
        slot.get(instance_id).cloned()
    };
    let Some(target) = target else {
        respond_error(stream, "404 Not Found", "unknown proxy instance");
        return Err(format!("unknown proxy instance: {instance_id}"));
    };

    // 重组请求头：剥离鉴权、重写 Host、强制 close。
    let mut content_length: usize = 0;
    let mut forward_head = String::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("authorization:")
            || lower.starts_with("host:")
            || lower.starts_with("connection:")
            || lower.starts_with("proxy-")
        {
            continue;
        }
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
        forward_head.push_str(line);
        forward_head.push_str("\r\n");
    }

    let target_path = format!("{}{rest_with_query}", target.path_prefix);
    let mut out_head = format!("{method} {target_path} HTTP/1.1\r\n");
    out_head.push_str(&format!("Host: {}:{}\r\n", target.host, target.port));
    out_head.push_str(&forward_head);
    out_head.push_str("Connection: close\r\n");
    out_head.push_str("\r\n");

    // localhost 在 macOS 上优先解析为 ::1，而 Ollama 等本地端点常只监听
    // 127.0.0.1——像 curl 一样逐地址回退，而不是只试第一个。
    let addrs = (target.host.as_str(), target.port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {}: {e}", target.host))?;
    let mut upstream: Option<TcpStream> = None;
    for addr in addrs {
        if let Ok(s) = TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
            upstream = Some(s);
            break;
        }
    }
    let mut upstream = match upstream {
        Some(s) => s,
        None => {
            respond_error(stream, "502 Bad Gateway", "upstream unreachable");
            return Err(format!(
                "connect upstream {}: all addresses refused",
                target.host
            ));
        }
    };
    upstream
        .set_read_timeout(Some(Duration::from_secs(600)))
        .ok();

    upstream
        .write_all(out_head.as_bytes())
        .map_err(|e| format!("write upstream head: {e}"))?;
    eprintln!(
        "[local-proxy] {method} /mbp/{instance_id}{rest_with_query} -> {}:{} (auth stripped)",
        target.host, target.port
    );

    // 请求体：预读部分 + 按 content-length 补足。
    let mut body_sent = 0usize;
    if body_sent < extra.len() {
        let take = (content_length - body_sent).min(extra.len() - body_sent);
        upstream
            .write_all(&extra[body_sent..body_sent + take])
            .map_err(|e| format!("write upstream body: {e}"))?;
        body_sent += take;
    }
    drop(extra);
    while body_sent < content_length {
        let mut chunk = vec![0u8; 8192.min(content_length - body_sent)];
        let n = stream
            .read(&mut chunk)
            .map_err(|e| format!("read request body: {e}"))?;
        if n == 0 {
            break;
        }
        upstream
            .write_all(&chunk[..n])
            .map_err(|e| format!("write upstream body: {e}"))?;
        body_sent += n;
    }

    // 响应：读取响应头并注入 Connection: close，随后原样泵送全部字节
    //（chunked / SSE 流式帧原样透传）。
    let mut resp_head: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if resp_head.len() > MAX_HEAD_BYTES {
            respond_error(stream, "502 Bad Gateway", "upstream head too large");
            return Err("upstream response head too large".into());
        }
        if let Some(pos) = find_subslice(&resp_head, b"\r\n\r\n") {
            let head_end = pos + 4;
            let mut head_text = String::from_utf8_lossy(&resp_head[..pos]).into_owned();
            if !head_text.to_ascii_lowercase().contains("connection:") {
                head_text.push_str("\r\nConnection: close");
            }
            let rebuilt = format!("{head_text}\r\n\r\n");
            stream
                .write_all(rebuilt.as_bytes())
                .map_err(|e| format!("write response head: {e}"))?;
            let tail = resp_head[head_end..].to_vec();
            stream
                .write_all(&tail)
                .map_err(|e| format!("write response tail: {e}"))?;
            break;
        }
        let n = upstream
            .read(&mut chunk)
            .map_err(|e| format!("read upstream head: {e}"))?;
        if n == 0 {
            return Err("upstream closed before response head".into());
        }
        resp_head.extend_from_slice(&chunk[..n]);
    }
    let mut pump = [0u8; 16384];
    loop {
        let n = match upstream.read(&mut pump) {
            Ok(0) => break,
            Ok(n) => n,
            // 读超时或中断：断开本次转发，不空转。
            Err(_) => break,
        };
        if stream.write_all(&pump[..n]).is_err() {
            break;
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_http_target_accepts_plain_and_port_and_path() {
        let target = parse_http_target("http://localhost:11434/v1/").unwrap();
        assert_eq!(target.host, "localhost");
        assert_eq!(target.port, 11434);
        assert_eq!(target.path_prefix, "/v1");

        let target = parse_http_target("http://10.0.0.7").unwrap();
        assert_eq!(target.port, 80);
        assert_eq!(target.path_prefix, "");

        assert!(parse_http_target("https://localhost:11434/v1").is_none());
        assert!(parse_http_target("").is_none());
    }

    #[test]
    fn proxy_base_url_shape() {
        assert_eq!(
            proxy_base_url(1234, "ollama"),
            "http://127.0.0.1:1234/mbp/ollama"
        );
    }

    #[test]
    fn is_local_endpoint_recognizes_private_and_rejects_cloud() {
        assert!(is_local_endpoint("http://localhost:11434/v1"));
        assert!(is_local_endpoint("http://127.0.0.1:8080"));
        assert!(is_local_endpoint("http://192.168.1.20:1234/v1"));
        assert!(is_local_endpoint("http://10.0.0.7"));
        assert!(is_local_endpoint("http://172.16.0.3:9/v1"));
        assert!(is_local_endpoint("http://172.31.255.1"));
        assert!(is_local_endpoint("http://nas.local:11434"));

        assert!(!is_local_endpoint("http://172.15.0.1"));
        assert!(!is_local_endpoint("http://172.32.0.1"));
        assert!(!is_local_endpoint("http://8.8.8.8:9"));
        assert!(!is_local_endpoint("https://api.deepseek.com/v1"));
        assert!(!is_local_endpoint("https://localhost:11434/v1"));
        assert!(!is_local_endpoint(""));
    }

    #[test]
    fn proxy_strips_authorization_and_relays_upstream() {
        let upstream = TcpListener::bind("127.0.0.1:0").unwrap();
        let up_addr = upstream.local_addr().unwrap();
        let up = thread::spawn(move || {
            let (mut s, _) = upstream.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut head = Vec::new();
            let mut tmp = [0u8; 256];
            let head_end = loop {
                let n = s.read(&mut tmp).unwrap();
                assert!(n > 0, "upstream got EOF before head");
                head.extend_from_slice(&tmp[..n]);
                if let Some(p) = find_subslice(&head, b"\r\n\r\n") {
                    break p + 4;
                }
            };
            let head_text = String::from_utf8_lossy(&head).into_owned();
            let mut body = head[head_end..].to_vec();
            while body.len() < 2 {
                let n = s.read(&mut tmp).unwrap();
                body.extend_from_slice(&tmp[..n]);
            }
            assert_eq!(body, b"{}", "request body must be forwarded");
            assert!(
                head_text.starts_with("POST /v1/chat/completions HTTP/1.1"),
                "path must be rewritten onto target prefix: {head_text}"
            );
            let lower = head_text.to_lowercase();
            assert!(
                !lower.contains("authorization:"),
                "authorization must be stripped: {head_text}"
            );
            assert!(
                lower.contains(&format!("host: localhost:{}", up_addr.port())),
                "host must point at the real target: {head_text}"
            );
            let payload = "data: ok\n\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{payload}",
                payload.len()
            );
            s.write_all(resp.as_bytes()).unwrap();
            let _ = s.shutdown(std::net::Shutdown::Both);
        });

        let proxied = set_proxy_targets(&[
            (
                "e2e".to_string(),
                format!("http://localhost:{}/v1", up_addr.port()),
            ),
            ("tls".to_string(), "https://private.example/v1".to_string()),
        ]);
        assert_eq!(proxied, vec!["e2e".to_string()], "https 不走代理");
        let port = ensure_proxy().unwrap();

        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let request = "POST /mbp/e2e/chat/completions HTTP/1.1\r\nHost: whatever\r\nAuthorization: Bearer sophonote-local\r\nContent-Length: 2\r\n\r\n{}";
        client.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.contains("data: ok"), "{response}");
        assert!(
            response.to_lowercase().contains("connection: close"),
            "{response}"
        );

        up.join().unwrap();
        set_proxy_targets(&[]);
    }
}
