use md5::{Digest, Md5};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
pub type Result<T> = std::result::Result<T, &'static str>;
pub const UA: &str = "Mozilla/5.0";
pub const REFERER: &str = "https://www.bilibili.com/";
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_connect(Some(Duration::from_secs(8)))
        .timeout_per_read(Some(Duration::from_secs(5)))
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .new_agent()
}
pub fn error(e: ureq::Error) -> &'static str {
    match e {
        ureq::Error::StatusCode(401 | 403) | ureq::Error::TooManyRedirects => {
            "source_access_denied"
        }
        ureq::Error::StatusCode(429) => "source_rate_limited",
        ureq::Error::Timeout(_) => "source_timeout",
        ureq::Error::BodyExceedsLimit(_) => "source_response_too_large",
        _ => "source_network_failed",
    }
}
pub fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err("cancelled")
    } else {
        Ok(())
    }
}
pub fn get(
    path: &str,
    params: &[(&str, String)],
    cookie: &str,
    cancel: &AtomicBool,
) -> Result<(Value, String)> {
    cancelled(cancel)?;
    let host = match path {
        "/x/passport-login/web/qrcode/generate" | "/x/passport-login/web/qrcode/poll" => {
            "https://passport.bilibili.com"
        }
        _ => "https://api.bilibili.com",
    };
    if !matches!(
        path,
        "/x/web-interface/nav"
            | "/x/web-interface/wbi/search/type"
            | "/x/web-interface/wbi/index/top/feed/rcmd"
            | "/x/polymer/web-dynamic/v1/feed/all"
            | "/x/web-interface/view"
            | "/x/player/wbi/playurl"
            | "/x/passport-login/web/qrcode/generate"
            | "/x/passport-login/web/qrcode/poll"
    ) {
        return Err("unsupported_method");
    }
    let mut request = agent()
        .get(format!("{host}{path}"))
        .header("User-Agent", UA)
        .header("Referer", REFERER)
        .header("Accept", "application/json");
    if !cookie.is_empty() {
        request = request.header("Cookie", cookie);
    }
    for (k, v) in params {
        request = request.query(*k, v);
    }
    let mut response = request.call().map_err(error)?;
    if response.status().is_redirection() {
        return Err("source_access_denied");
    }
    let cookies = cookies(
        response
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|h| h.to_str().ok()),
    )?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_vec()
        .map_err(error)?;
    cancelled(cancel)?;
    if bytes.is_empty() {
        return Err("source_empty_response");
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid_source_response")?;
    match v["code"].as_i64().ok_or("invalid_source_response")? {
        0 => {}
        -101 if path == "/x/web-interface/nav" => {}
        -101 => return Err("account_required"),
        -412 | -352 => return Err("source_verification_required"),
        _ => return Err("source_request_rejected"),
    }
    Ok((v, cookies))
}
fn cookies<'a>(values: impl Iterator<Item = &'a str>) -> Result<String> {
    let mut out = BTreeMap::new();
    for v in values {
        let Some((name, value)) = v.split(';').next().and_then(|s| s.split_once('=')) else {
            continue;
        };
        if matches!(
            name,
            "SESSDATA" | "bili_jct" | "DedeUserID" | "DedeUserID__ckMd5"
        ) {
            if value.is_empty()
                || value.len() > 2048
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_graphic() && !b";\r\n".contains(&b))
            {
                return Err("invalid_source_response");
            }
            out.insert(name, value);
        }
    }
    let s = out
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    if s.len() > 4096 {
        return Err("invalid_source_response");
    }
    Ok(s)
}
pub fn text<'a>(v: &'a Value, key: &str, max: usize) -> Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| {
            s.chars().count() <= max
                && !s
                    .chars()
                    .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        })
        .ok_or("invalid_source_response")
}
pub fn bvid(s: &str) -> bool {
    s.len() == 12 && s.starts_with("BV") && s.bytes().all(|b| b.is_ascii_alphanumeric())
}
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn sign(
    mut params: Vec<(&'static str, String)>,
    key: &str,
    ts: u64,
) -> Vec<(&'static str, String)> {
    params.push(("wts", ts.to_string()));
    for (_, v) in &mut params {
        v.retain(|c| !"!'()*".contains(c));
    }
    params.sort_by_key(|(k, _)| *k);
    let q = params
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    params.push((
        "w_rid",
        format!("{:x}", Md5::digest(format!("{q}{key}").as_bytes())),
    ));
    params
}
pub fn wbi(cookie: &str, cancel: &AtomicBool) -> Result<String> {
    let (v, _) = get("/x/web-interface/nav", &[], cookie, cancel)?;
    let img = &v["data"]["wbi_img"];
    let mut all = String::new();
    for k in ["img_url", "sub_url"] {
        let url = text(img, k, 1024).map_err(|_| "wbi_key_unavailable")?;
        let name = url
            .rsplit('/')
            .next()
            .and_then(|s| s.split('.').next())
            .filter(|s| s.len() == 32 && s.bytes().all(|c| c.is_ascii_hexdigit()))
            .ok_or("invalid_source_response")?;
        all.push_str(name);
    }
    let table = [
        46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19,
        29, 28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4,
        22, 25, 54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
    ];
    Ok(table[..32]
        .iter()
        .map(|i| all.as_bytes()[*i] as char)
        .collect())
}
pub fn signed(
    path: &str,
    p: Vec<(&'static str, String)>,
    cookie: &str,
    cancel: &AtomicBool,
) -> Result<Value> {
    let key = wbi(cookie, cancel)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "clock_failed")?
        .as_secs();
    Ok(get(path, &sign(p, &key, ts), cookie, cancel)?.0["data"].clone())
}
pub fn search(query: &str, page: u64, cookie: &str, cancel: &AtomicBool) -> Result<Value> {
    if query.trim().is_empty()
        || query.chars().count() > 100
        || query.chars().any(char::is_control)
        || !(1..=1000).contains(&page)
    {
        return Err("invalid_arguments");
    }
    let data = signed(
        "/x/web-interface/wbi/search/type",
        vec![
            ("search_type", "video".into()),
            ("keyword", query.trim().into()),
            ("page", page.to_string()),
        ],
        cookie,
        cancel,
    )?;
    decode_search(&data, page)
}
fn decode_search(data: &Value, page: u64) -> Result<Value> {
    let items = data["result"]
        .as_array()
        .filter(|a| a.len() <= 50)
        .ok_or("invalid_source_response")?;
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for i in items {
        if i["type"] != "video" {
            continue;
        }
        let id = text(i, "bvid", 12).map_err(|_| "invalid_search_video_id")?;
        if !bvid(id) {
            return Err("invalid_search_video_id");
        }
        if !seen.insert(id) {
            continue;
        }
        let raw = text(i, "title", 2000).map_err(|_| "invalid_search_title")?;
        let title = scraper::Html::parse_fragment(raw)
            .root_element()
            .text()
            .collect::<String>();
        out.push(json!({"id":id,"title":title,"author":text(i,"author",200).map_err(|_|"invalid_search_author")?,"duration":text(i,"duration",32).map_err(|_|"invalid_search_duration")?}));
    }
    let pages = data["numPages"]
        .as_u64()
        .filter(|n| *n <= 1000)
        .ok_or("invalid_source_response")?;
    Ok(json!({"items":out,"page":page,"has_more":page<pages}))
}
pub struct Media {
    pub url: String,
    pub size: u64,
    pub title: String,
    pub duration: f64,
    pub quality: u64,
    pub bvid: String,
    pub cid: String,
    pub entries: Value,
}
pub fn media(
    id: &str,
    selected_cid: Option<&str>,
    cookie: &str,
    cancel: &AtomicBool,
) -> Result<Media> {
    if !bvid(id) {
        return Err("invalid_arguments");
    }
    let (v, _) = get(
        "/x/web-interface/view",
        &[("bvid", id.into())],
        cookie,
        cancel,
    )?;
    let view = &v["data"];
    if view["bvid"] != id {
        return Err("invalid_source_response");
    }
    let page = view["pages"]
        .as_array()
        .and_then(|a| match selected_cid {
            Some(cid) => a
                .iter()
                .find(|p| p["cid"].as_u64().is_some_and(|n| n.to_string() == cid)),
            None => a.first(),
        })
        .ok_or("invalid_episode")?;
    let cid = page["cid"]
        .as_u64()
        .ok_or("invalid_source_response")?
        .to_string();
    let duration = page["duration"]
        .as_f64()
        .filter(|n| *n > 0. && *n <= 86400.)
        .ok_or("invalid_source_response")?;
    let d = signed(
        "/x/player/wbi/playurl",
        vec![
            ("bvid", id.into()),
            ("cid", cid.clone()),
            ("qn", "32".into()),
            ("fnval", "1".into()),
            ("fnver", "0".into()),
            ("fourk", "0".into()),
        ],
        cookie,
        cancel,
    )?;
    if d["is_preview"] == true || d["is_preview"] == 1 {
        return Err("source_incomplete_media");
    }
    if (d["timelength"].as_f64().ok_or("invalid_source_response")? / 1000. - duration).abs() > 3. {
        return Err("source_incomplete_media");
    }
    let durl = d["durl"]
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or("unsupported_media_format")?;
    let src = &durl[0];
    let url = media_url(src)?;
    let size = src["size"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 16 * 1024 * 1024 * 1024)
        .ok_or("invalid_source_response")?;
    Ok(Media {
        url,
        size,
        title: text(view, "title", 300)?.into(),
        duration,
        quality: d["quality"].as_u64().ok_or("invalid_source_response")?,
        bvid: id.into(),
        cid,
        entries: crate::feed::entries(view)?,
    })
}
fn media_url(src: &Value) -> Result<String> {
    let primary = text(src, "url", 8192)?;
    if cdn(primary) {
        return Ok(primary.into());
    }
    if let Some(backups) = src.get("backup_url").filter(|v| !v.is_null()) {
        let backups = backups
            .as_array()
            .filter(|a| a.len() <= 8)
            .ok_or("invalid_source_response")?;
        for value in backups {
            let url = value
                .as_str()
                .filter(|s| s.len() <= 8192)
                .ok_or("invalid_source_response")?;
            if cdn(url) {
                return Ok(url.into());
            }
        }
    }
    Err("source_cdn_rejected")
}
pub fn cdn(url: &str) -> bool {
    url.parse::<ureq::http::Uri>().is_ok_and(|u| {
        u.scheme_str() == Some("https")
            && u.authority()
                .is_some_and(|a| a.port_u16().is_none() && !a.as_str().contains('@'))
            && u.host()
                .is_some_and(|h| h.ends_with(".bilivideo.com") || h.ends_with(".bilivideo.cn"))
    })
}
pub fn identity(cookie: &str, cancel: &AtomicBool) -> Result<String> {
    let (v, _) = get("/x/web-interface/nav", &[], cookie, cancel)?;
    if v["code"] != 0 || v["data"]["isLogin"] != true {
        return Err("account_expired");
    }
    Ok(text(&v["data"], "uname", 200)?.into())
}
pub fn qr(cancel: &AtomicBool) -> Result<(String, String)> {
    use base64::Engine as _;
    let (v, _) = get("/x/passport-login/web/qrcode/generate", &[], "", cancel)?;
    let d = &v["data"];
    let key = text(d, "qrcode_key", 128)?.to_owned();
    let url = text(d, "url", 2048)?;
    if key.is_empty()
        || !key.bytes().all(|b| b.is_ascii_alphanumeric())
        || !url.parse::<ureq::http::Uri>().is_ok_and(|u| {
            u.scheme_str() == Some("https")
                && matches!(
                    u.host(),
                    Some("passport.bilibili.com" | "account.bilibili.com")
                )
                && u.authority()
                    .is_some_and(|a| a.port_u16().is_none() && !a.as_str().contains('@'))
        })
    {
        return Err("invalid_source_response");
    }
    let qr = qrcodegen::QrCode::encode_text(url, qrcodegen::QrCodeEcc::Medium)
        .map_err(|_| "qr_failed")?;
    let n = qr.size() + 8;
    let mut svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 {n} {n}' shape-rendering='crispEdges'><path fill='white' d='M0 0h{n}v{n}H0z'/><path fill='black' d='"
    );
    for y in 0..qr.size() {
        for x in 0..qr.size() {
            if qr.get_module(x, y) {
                svg.push_str(&format!("M{} {}h1v1h-1z", x + 4, y + 4));
            }
        }
    }
    svg.push_str("'/></svg>");
    Ok((
        key,
        format!(
            "data:image/svg+xml;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(svg)
        ),
    ))
}
pub enum Login {
    Pending(&'static str),
    Confirmed { cookie: String, name: String },
}
pub fn poll(key: &str, cancel: &AtomicBool) -> Result<Login> {
    let (v, cookie) = get(
        "/x/passport-login/web/qrcode/poll",
        &[("qrcode_key", key.into())],
        "",
        cancel,
    )?;
    match v["data"]["code"].as_i64() {
        Some(86101) => Ok(Login::Pending("pending")),
        Some(86090) => Ok(Login::Pending("scanned")),
        Some(86038) => Err("qr_expired"),
        Some(0) => {
            if !cookie.contains("SESSDATA=") {
                return Err("invalid_source_response");
            }
            let name = identity(&cookie, cancel)?;
            Ok(Login::Confirmed { cookie, name })
        }
        _ => Err("invalid_source_response"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn course_cards_do_not_break_video_search_and_titles_are_plain_text() {
        let data = json!({"numPages":2,"result":[{"type":"ketang","title":"付费课程"},{"type":"video","bvid":"BV1oJ41137oE","title":"<em>标题</em> &amp; 示例","author":"UP","duration":"1:03"}]});
        let out = decode_search(&data, 1).unwrap();
        assert_eq!(out["items"].as_array().unwrap().len(), 1);
        assert_eq!(out["items"][0]["title"], "标题 & 示例");
        assert_eq!(out["has_more"], true);
        assert!(decode_search(&json!({"numPages":1}), 1).is_err());
    }
    #[test]
    fn choose_official_compatible_resource_before_any_network_request() {
        let source = json!({"url":"https://peer.bilivideo.cn:4483/media","backup_url":["https://upos.bilivideo.com/media?opaque=1"]});
        assert_eq!(
            media_url(&source).unwrap(),
            "https://upos.bilivideo.com/media?opaque=1"
        );
        assert!(
            media_url(&json!({"url":"https://peer.bilivideo.cn:4483/media","backup_url":[]}))
                .is_err()
        );
        assert!(media_url(&json!({"url":"https://evil.test/x","backup_url":["https://bilivideo.com.evil.test/x"]})).is_err());
    }
    #[test]
    fn cdn_boundary() {
        assert!(cdn("https://upos-sz-mirror.bilivideo.com/media?token=x"));
        for u in [
            "http://upos.bilivideo.com/x",
            "https://bilivideo.com.evil.test/x",
            "https://a@upos.bilivideo.com/x",
            "https://upos.bilivideo.com:443/x",
        ] {
            assert!(!cdn(u));
        }
    }
    #[test]
    fn cookie_allowlist() {
        assert_eq!(
            cookies(["SESSDATA=opaque; HttpOnly; Secure", "tracking=no; path=/"].into_iter())
                .unwrap(),
            "SESSDATA=opaque"
        );
        assert!(cookies(["SESSDATA=x\r\ny"].into_iter()).is_err());
    }
    #[test]
    fn signing_is_deterministic() {
        let p = sign(vec![("keyword", "a !b".into())], "key", 1);
        assert_eq!(p[0], ("keyword", "a b".into()));
        assert_eq!(
            p.last().unwrap().1,
            format!("{:x}", Md5::digest(b"keyword=a%20b&wts=1key"))
        );
        assert!(bvid("BV1oJ41137oE"));
        assert!(!bvid("../BV1oJ41137oE"));
    }
}
