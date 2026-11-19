use crate::api::{self, Result};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::atomic::AtomicBool};

pub fn cursor(s: &str) -> bool {
    s.len() <= 128 && s.bytes().all(|b| b.is_ascii_digit())
}
fn array(v: &Value, max: usize) -> Result<&Vec<Value>> {
    v.as_array()
        .filter(|a| a.len() <= max)
        .ok_or("invalid_source_response")
}
fn seconds(n: u64) -> String {
    format!("{}:{:02}", n / 60, n % 60)
}
fn card(id: &str, title: &str, author: &str, duration: String) -> Result<Value> {
    if !api::bvid(id) {
        return Err("invalid_source_response");
    }
    Ok(json!({"id":id,"title":title,"author":author,"duration":duration}))
}
pub fn recommend(fresh: u64, cookie: &str, cancel: &AtomicBool) -> Result<Value> {
    let data = api::signed(
        "/x/web-interface/wbi/index/top/feed/rcmd",
        vec![
            ("fresh_type", "4".into()),
            ("ps", "12".into()),
            ("fresh_idx", fresh.to_string()),
            ("fresh_idx_1h", fresh.to_string()),
            ("brush", fresh.to_string()),
            ("fetch_row", "1".into()),
        ],
        cookie,
        cancel,
    )?;
    decode_recommend(&data)
}
fn decode_recommend(data: &Value) -> Result<Value> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for i in array(&data["item"], 50)? {
        if i["goto"] != "av" || !i["business_info"].is_null() {
            continue;
        }
        let id = api::text(i, "bvid", 12)?;
        if !seen.insert(id) {
            continue;
        }
        out.push(card(
            id,
            api::text(i, "title", 300)?,
            api::text(&i["owner"], "name", 200)?,
            seconds(i["duration"].as_u64().ok_or("invalid_source_response")?),
        )?);
    }
    Ok(json!({"items":out,"has_more":false}))
}
pub fn following(offset: &str, cookie: &str, cancel: &AtomicBool) -> Result<Value> {
    if cookie.is_empty() {
        return Err("account_required");
    }
    if !cursor(offset) {
        return Err("invalid_arguments");
    }
    let mut params = vec![("type", "video".into())];
    if !offset.is_empty() {
        params.push(("offset", offset.into()));
    }
    let (data, _) = api::get(
        "/x/polymer/web-dynamic/v1/feed/all",
        &params,
        cookie,
        cancel,
    )?;
    decode_following(&data["data"])
}
fn decode_following(data: &Value) -> Result<Value> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for i in array(&data["items"], 50)? {
        if i["type"] != "DYNAMIC_TYPE_AV" {
            continue;
        }
        let modules = &i["modules"];
        let a = &modules["module_dynamic"]["major"]["archive"];
        let id = api::text(a, "bvid", 12)?;
        if !seen.insert(id) {
            continue;
        }
        out.push(card(
            id,
            api::text(a, "title", 300)?,
            api::text(&modules["module_author"], "name", 200)?,
            api::text(a, "duration_text", 32)?.into(),
        )?);
    }
    let more = match &data["has_more"] {
        Value::Bool(v) => *v,
        Value::Number(n) if n.as_u64() == Some(0) => false,
        Value::Number(n) if n.as_u64() == Some(1) => true,
        _ => return Err("invalid_source_response"),
    };
    let offset = api::text(data, "offset", 128)?;
    if !cursor(offset) || more && offset.is_empty() {
        return Err("invalid_source_response");
    }
    Ok(json!({"items":out,"has_more":more,"offset":offset}))
}
pub fn entries(view: &Value) -> Result<Value> {
    let id = api::text(view, "bvid", 12)?;
    let title = api::text(view, "title", 300)?;
    let pages = array(&view["pages"], 2000)?;
    if pages.is_empty() {
        return Err("invalid_source_response");
    }
    let mut out = Vec::new();
    if pages.len() > 1 || view["ugc_season"].is_null() {
        for p in pages {
            let cid = p["cid"]
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or("invalid_source_response")?;
            out.push(json!({"bvid":id,"cid":cid.to_string(),"title":api::text(p,"part",300)?}));
        }
    } else {
        let season = &view["ugc_season"];
        for section in array(&season["sections"], 2000)? {
            for e in array(&section["episodes"], 2000)? {
                if out.len() >= 2000 {
                    return Err("catalog_too_large");
                }
                let bvid = api::text(e, "bvid", 12)?;
                if !api::bvid(bvid) {
                    return Err("invalid_source_response");
                }
                let cid = e["cid"]
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or("invalid_source_response")?;
                out.push(
                    json!({"bvid":bvid,"cid":cid.to_string(),"title":api::text(e,"title",300)?}),
                );
            }
        }
        if season["ep_count"].as_u64() != Some(out.len() as u64) {
            return Ok(json!({"title":title,"items":[],"complete":false}));
        }
    }
    let mut seen = BTreeSet::new();
    if !out
        .iter()
        .all(|e| seen.insert((e["bvid"].to_string(), e["cid"].to_string())))
    {
        return Err("invalid_source_response");
    }
    Ok(json!({"title":title,"items":out,"complete":true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_non_video_and_promoted_cards() {
        let card = json!({"goto":"av","bvid":"BV1oJ41137oE","title":"标题","owner":{"name":"UP"},"duration":65});
        let mut ad = card.clone();
        ad["business_info"] = json!({"ad":true});
        let v = decode_recommend(&json!({"item":[{"goto":"live"},ad,card.clone(),card]})).unwrap();
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        assert_eq!(v["items"][0]["duration"], "1:05");
        assert!(decode_following(&json!({})).is_err());
        assert!(!cursor("x&token=secret"));
    }
    #[test]
    fn catalog_requires_complete_counts_and_unique_entries() {
        let base = json!({"bvid":"BV1oJ41137oE","title":"视频","pages":[{"cid":1,"part":"1"},{"cid":2,"part":"2"}]});
        assert_eq!(
            entries(&base).unwrap()["items"].as_array().unwrap().len(),
            2
        );
        let mut duplicate = base.clone();
        duplicate["pages"][1]["cid"] = json!(1);
        assert!(entries(&duplicate).is_err());
        let mut partial = base;
        partial["pages"] = json!([{"cid":1,"part":"1"}]);
        partial["ugc_season"] = json!({"ep_count":2,"sections":[{"episodes":[{"cid":1,"bvid":"BV1oJ41137oE","title":"1"}]}]});
        assert_eq!(entries(&partial).unwrap()["complete"], false);
    }
}
