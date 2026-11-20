mod api;
mod feed;
mod resource;
mod vault;
use api::Result;
use serde_json::{Value, json};
use std::{
    io::{BufRead, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};
enum Outcome {
    Json(Value),
    Media(api::Media, String),
    Qr(String, String),
    Identity(String, String),
    Poll(api::Login),
}
struct Task {
    id: u64,
    method: String,
    cancel: Arc<AtomicBool>,
    worker: Option<JoinHandle<(Result<Outcome>, Value)>>,
    result: Option<Result<Value>>,
    diagnostic: Value,
}
struct Engine {
    server: resource::Server,
    task: Option<Task>,
    next: u64,
    cookie: String,
    name: String,
    qr: Option<String>,
    qr_image: String,
    attempt: u64,
    account_state: &'static str,
}
impl Engine {
    fn new() -> Result<Self> {
        Ok(Self {
            server: resource::Server::new()?,
            task: None,
            next: 0,
            cookie: String::new(),
            name: String::new(),
            qr: None,
            qr_image: String::new(),
            attempt: 0,
            account_state: "idle",
        })
    }
    fn cancel(&self) {
        if let Some(t) = &self.task {
            t.cancel.store(true, Ordering::Release);
        }
    }
    fn account(&self) -> Value {
        json!({"state":self.account_state,"name":self.name,"qr_image":self.qr_image,"attempt":self.attempt})
    }
    fn clear_qr(&mut self) {
        self.qr = None;
        self.qr_image.clear();
        self.attempt = 0;
        self.account_state = if self.cookie.is_empty() {
            "idle"
        } else {
            "signed_in"
        };
    }
    fn begin(
        &mut self,
        method: &str,
        work: impl FnOnce(Arc<AtomicBool>) -> Result<Outcome> + Send + 'static,
    ) -> Result<Value> {
        if self
            .task
            .as_ref()
            .is_some_and(|t| t.worker.as_ref().is_some_and(|w| !w.is_finished()))
        {
            return Err("request_busy");
        }
        self.cancel();
        self.next += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let worker = std::thread::spawn(move || {
            let result = work(c);
            (result, api::take_diagnostic())
        });
        self.task = Some(Task {
            id: self.next,
            method: method.into(),
            cancel,
            worker: Some(worker),
            result: None,
            diagnostic: Value::Null,
        });
        Ok(json!({"job":self.next,"status":"pending"}))
    }
    fn status(&mut self, id: u64) -> Result<Value> {
        let t = self
            .task
            .as_mut()
            .filter(|t| t.id == id)
            .ok_or("job_not_found")?;
        if t.cancel.load(Ordering::Acquire) {
            return Ok(json!({"job":id,"status":"cancelled"}));
        }
        if let Some(w) = t.worker.as_ref() {
            if !w.is_finished() {
                return Ok(json!({"job":id,"status":"pending"}));
            }
            let (result, diagnostic) = t
                .worker
                .take()
                .unwrap()
                .join()
                .unwrap_or((Err("service_worker_failed"), Value::Null));
            t.diagnostic = diagnostic;
            let result=result.and_then(|out|match out {
    Outcome::Json(v)=>Ok(v),
    Outcome::Media(m,o)=>self.server.open(m,o,id),
    Outcome::Qr(key,image)=>{self.qr=Some(key);self.qr_image=image;self.attempt=id;self.account_state="pending";Ok(json!({"state":self.account_state,"name":self.name,"qr_image":self.qr_image,"attempt":self.attempt}))},
    Outcome::Identity(cookie,name)=>{self.cookie=cookie;self.name=name;self.account_state=if self.cookie.is_empty(){"idle"}else{"signed_in"};Ok(json!({"state":self.account_state,"name":self.name,"qr_image":"","attempt":0}))},
    Outcome::Poll(api::Login::Pending(state))=>{self.account_state=state;Ok(json!({"state":state,"name":self.name,"qr_image":self.qr_image,"attempt":self.attempt}))},
    Outcome::Poll(api::Login::Confirmed{cookie,name})=>{vault::write(&cookie)?;self.cookie=cookie;self.name=name;self.qr=None;self.qr_image.clear();self.attempt=0;self.account_state="signed_in";Ok(json!({"state":"signed_in","name":self.name,"qr_image":"","attempt":0}))},
   });
            if matches!(result, Err("account_expired" | "account_required")) {
                self.cookie.clear();
                self.name.clear();
                self.qr = None;
                self.qr_image.clear();
                self.attempt = 0;
                self.account_state = "expired";
            }
            t.result = Some(result);
        }
        match t.result.as_ref().ok_or("job_not_found")? {
            Ok(v) => Ok(json!({"job":id,"status":"complete","result":v})),
            Err(e) => Ok(json!({"job":id,"status":"failed","error":e,"diagnostic":t.diagnostic})),
        }
    }
    fn call(&mut self, method: &str, args: &Value) -> Result<Value> {
        match method {
            "health" => Ok(
                json!({"service":"bilibili","api_version":1,"version":env!("CARGO_PKG_VERSION"),"capabilities":["search","qr_login","browser_resources","recommend","following","episodes"],"media":self.server.counters()}),
            ),
            "task.status" => self.status(args["job"].as_u64().ok_or("invalid_arguments")?),
            "task.cancel" => {
                let id = args["job"].as_u64().ok_or("invalid_arguments")?;
                if self.task.as_ref().is_none_or(|t| t.id != id) {
                    return Err("job_not_found");
                }
                self.cancel();
                Ok(json!({}))
            }
            "search" => {
                let query = api::text(args, "query", 100)?.trim().to_string();
                let page = args
                    .get("page")
                    .map(|v| v.as_u64().ok_or("invalid_arguments"))
                    .transpose()?
                    .unwrap_or(1);
                if query.is_empty()
                    || query.chars().any(char::is_control)
                    || !(1..=1000).contains(&page)
                {
                    return Err("invalid_arguments");
                }
                let cookie = self.cookie.clone();
                self.begin(method, move |c| {
                    Ok(Outcome::Json(api::search(&query, page, &cookie, &c)?))
                })
            }
            "recommend" | "following" => {
                let cookie = self.cookie.clone();
                if method == "following" && cookie.is_empty() {
                    return Err("account_required");
                }
                let fresh = args.get("fresh").and_then(Value::as_u64).unwrap_or(1);
                let offset = args
                    .get("offset")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if !(1..=1_000_000).contains(&fresh) || !feed::cursor(&offset) {
                    return Err("invalid_arguments");
                }
                let following = method == "following";
                self.begin(method, move |c| {
                    Ok(Outcome::Json(if following {
                        feed::following(&offset, &cookie, &c)?
                    } else {
                        feed::recommend(fresh, &cookie, &c)?
                    }))
                })
            }
            "account.get" => Ok(self.account()),
            "account.restore" => self.begin(method, move |c| {
                let cookie = vault::read()?.unwrap_or_default();
                let name = if cookie.is_empty() {
                    String::new()
                } else {
                    api::identity(&cookie, &c)?
                };
                api::cancelled(&c)?;
                Ok(Outcome::Identity(cookie, name))
            }),
            "account.login" => {
                if self.account_state == "signed_in" {
                    return Err("already_signed_in");
                }
                let reply = self.begin(method, move |c| {
                    let (k, image) = api::qr(&c)?;
                    Ok(Outcome::Qr(k, image))
                })?;
                self.clear_qr();
                self.attempt = self.next;
                self.account_state = "preparing";
                Ok(reply)
            }
            "account.poll" => {
                if args["attempt"].as_u64() != Some(self.attempt) {
                    return Err("login_not_found");
                }
                let key = self.qr.clone().ok_or("login_not_found")?;
                self.begin(method, move |c| Ok(Outcome::Poll(api::poll(&key, &c)?)))
            }
            "account.cancel" => {
                let attempt = args["attempt"].as_u64().ok_or("invalid_arguments")?;
                if attempt != self.attempt {
                    return Err("login_not_found");
                }
                if self
                    .task
                    .as_ref()
                    .is_some_and(|t| t.method.starts_with("account."))
                {
                    self.cancel();
                }
                self.clear_qr();
                Ok(self.account())
            }
            "account.logout" => {
                vault::delete()?;
                self.cancel();
                self.server.close();
                self.cookie.clear();
                self.name.clear();
                self.clear_qr();
                Ok(self.account())
            }
            "playback.open" => {
                let id = api::text(args, "bvid", 12)?.to_owned();
                let origin = api::text(args, "origin", 200)?.to_owned();
                if !api::bvid(&id) || !resource::origin(&origin) {
                    return Err("invalid_arguments");
                }
                let cid = args
                    .get("cid")
                    .map(|v| {
                        v.as_str()
                            .filter(|s| {
                                !s.is_empty()
                                    && s.len() <= 20
                                    && s.bytes().all(|b| b.is_ascii_digit())
                            })
                            .ok_or("invalid_arguments")
                    })
                    .transpose()?
                    .map(str::to_owned);
                let cookie = self.cookie.clone();
                let reply = self.begin(method, move |c| {
                    Ok(Outcome::Media(
                        api::media(&id, cid.as_deref(), &cookie, &c)?,
                        origin,
                    ))
                })?;
                self.server.close();
                Ok(reply)
            }
            "playback.suspend" | "playback.resume" => self.server.control(
                args["session"].as_u64().ok_or("invalid_arguments")?,
                method == "playback.resume",
            ),
            "playback.close" => {
                let id = args["session"].as_u64().ok_or("invalid_arguments")?;
                self.server.control(id, false)?;
                self.server.close();
                Ok(json!({}))
            }
            "lifecycle.suspend" => {
                self.cancel();
                if args["release"] == true {
                    self.server.close();
                } else {
                    self.server.suspend();
                }
                self.clear_qr();
                Ok(json!({"suspended":true}))
            }
            _ => Err("unsupported_method"),
        }
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.cancel();
        self.server.close();
        if let Some(t) = self.task.as_mut() {
            if let Some(w) = t.worker.take() {
                let _ = w.join();
            }
        }
    }
}
fn main() {
    let mut engine = match Engine::new() {
        Ok(e) => e,
        Err(_) => return,
    };
    let stdin = std::io::stdin();
    let mut reader = std::io::BufReader::new(stdin.lock());
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader
            .by_ref()
            .take(65537)
            .read_until(b'\n', &mut line)
            .unwrap_or(0)
            == 0
        {
            break;
        }
        if line.len() > 65536 {
            break;
        }
        let Ok(v) = serde_json::from_slice::<Value>(&line) else {
            break;
        };
        let result = if v["version"] != 1 || !v["args"].is_object() {
            Err("invalid_arguments")
        } else {
            engine.call(v["method"].as_str().unwrap_or(""), &v["args"])
        };
        let reply = match result {
            Ok(data) => json!({"version":1,"id":v["id"],"ok":true,"data":data}),
            Err(code) => json!({"version":1,"id":v["id"],"ok":false,"error":{"code":code}}),
        };
        let out = std::io::stdout();
        let mut out = out.lock();
        if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_worker_returns_only_bounded_network_diagnostics() {
        let mut e = Engine::new().unwrap();
        let j = e
            .begin("search", |_| {
                Err(api::error(ureq::Error::ConnectProxyFailed(
                    "secret proxy URL".into(),
                )))
            })
            .unwrap();
        while !e
            .task
            .as_ref()
            .unwrap()
            .worker
            .as_ref()
            .unwrap()
            .is_finished()
        {
            std::thread::yield_now();
        }
        let result = e.status(j["job"].as_u64().unwrap()).unwrap();
        assert_eq!(result["error"], "source_proxy_failed");
        assert_eq!(result["diagnostic"], json!({"exception_type":"proxy"}));
        assert!(!result.to_string().contains("secret"));
        let j = e
            .begin("search", |_| Err("invalid_source_response"))
            .unwrap();
        while !e
            .task
            .as_ref()
            .unwrap()
            .worker
            .as_ref()
            .unwrap()
            .is_finished()
        {
            std::thread::yield_now();
        }
        assert!(e.status(j["job"].as_u64().unwrap()).unwrap()["diagnostic"].is_null());
    }
    #[test]
    fn cancelled_work_counts_until_finished_and_cannot_commit() {
        let mut e = Engine::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let j = e
            .begin("search", move |_| {
                rx.recv().unwrap();
                Ok(Outcome::Json(json!({"late":true})))
            })
            .unwrap();
        e.call("lifecycle.suspend", &json!({})).unwrap();
        assert_eq!(
            e.status(j["job"].as_u64().unwrap()).unwrap()["status"],
            "cancelled"
        );
        assert_eq!(
            e.begin("search", move |_| Ok(Outcome::Json(json!({})))),
            Err("request_busy")
        );
        tx.send(()).unwrap();
    }
    #[test]
    fn busy_login_does_not_destroy_current_attempt() {
        let mut e = Engine::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        e.begin("search", move |_| {
            rx.recv().unwrap();
            Ok(Outcome::Json(json!({})))
        })
        .unwrap();
        e.qr = Some("current".into());
        e.attempt = 9;
        assert_eq!(e.call("account.login", &json!({})), Err("request_busy"));
        assert_eq!(e.qr.as_deref(), Some("current"));
        assert_eq!(e.attempt, 9);
        tx.send(()).unwrap();
    }
    #[test]
    fn validation_precedes_network() {
        let mut e = Engine::new().unwrap();
        assert!(e.call("search", &json!({"query":"","page":1})).is_err());
        assert!(
            e.call(
                "playback.open",
                &json!({"bvid":"../x","origin":"https://evil.test"})
            )
            .is_err()
        );
        assert!(e.call("account.poll", &json!({"attempt":4})).is_err());
        assert!(e.task.is_none());
    }
}
