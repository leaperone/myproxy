//! Small, stable JSON facade for Expo native modules and the Packet Tunnel.
//! All routing and node selection is delegated to `myproxy-core`.
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};
use anyhow::{bail, Context, Result};
use myproxy_core::{Catalog, Group, Node, NodeHealth, RuleSet, Strategy, Subscription};
use serde_json::{json, Value};

struct Runtime { strategy: Strategy, catalog: Catalog, health: HashMap<String, NodeHealth>, platform: String, revision: u64 }
#[derive(serde::Serialize, serde::Deserialize)] struct Document { strategy: Strategy, catalog: Catalog }
static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();

fn runtime() -> &'static Mutex<Runtime> { RUNTIME.get_or_init(|| Mutex::new(Runtime::new("android"))) }
fn default_strategy() -> Strategy {
    let mut s = myproxy_core::xray::default_strategy();
    /* Keep this adapter local only to preserve the mobile ownership boundary. */
    s.rule_sets.clear();
    s
}
impl Runtime {
    fn new(platform: &str) -> Self { Self { strategy: default_strategy(), catalog: Catalog::default(), health: HashMap::new(), platform: platform.into(), revision: 1 } }
    fn snapshot(&self) -> Value {
        let groups = self.strategy.groups.iter().map(|g| {
            let members = myproxy_core::catalog::resolve_group_members(g, &self.catalog).into_iter().map(|name| json!({"name":name,"kind":"node","available":true,"delayMs":self.health.get(&name).and_then(|h| h.delay_ms)})).collect::<Vec<_>>();
            json!({"id":g.id,"name":g.name,"kind":g.kind,"selected":g.selected,"resolved":g.selected,"members":members})
        }).collect::<Vec<_>>();
        let rules = self.strategy.rule_sets.iter().map(|r| json!({"id":r.id,"name":r.name,"kind":r.matchers.first().map(|m|m.kind.clone()).unwrap_or_default(),"value":r.matchers.first().map(|m|m.value.clone()).unwrap_or_default(),"via":r.via})).collect::<Vec<_>>();
        json!({"revision":self.revision,"mode":if self.strategy.mixed_mode == myproxy_core::InboundMode::Global {"global"} else {self.strategy.mixed_mode.as_str()},"autoConnect":self.strategy.connect_on_launch,"selected":self.strategy.global_selected,"fallback":self.strategy.unmatched_via,"capabilities":{"dynamicDirect":self.platform=="android","appRouting":false},"sources":self.strategy.subscriptions.iter().map(|s|json!({"id":s.id,"name":s.name,"url":s.url,"nodeCount":self.catalog.nodes.iter().filter(|n|n.subscription==s.name).count()})).collect::<Vec<_>>(),"nodes":self.catalog.nodes.iter().map(|n|json!({"name":n.name,"protocol":n.raw.get("type").and_then(|v|v.as_str()).unwrap_or("unknown"),"source":n.subscription,"available":true,"error":Value::Null,"delayMs":self.health.get(&n.name).and_then(|h|h.delay_ms)})).collect::<Vec<_>>(),"groups":groups,"rules":rules,"warnings":self.catalog.refresh_warnings(),"runtime":{"phase":"disconnected","message":Value::Null,"connectedAt":Value::Null,"uploadBytes":0,"downloadBytes":0,"connections":[]}})
    }
}
fn ok(data: Value) -> String { json!({"ok":true,"data":data}).to_string() }
fn err(code: &str, message: impl Into<String>) -> String { json!({"ok":false,"error":{"code":code,"message":message.into()}}).to_string() }
fn parse_node(raw: serde_yaml::Value, source: &str, fallback: &str) -> Result<Node> {
    let name = raw.get("name").and_then(|v|v.as_str()).unwrap_or(fallback).trim().to_string();
    if name.is_empty() { bail!("节点缺少名称") }
    Ok(Node { name, subscription: source.into(), raw })
}
fn validate_mobile_strategy(strategy: &Strategy, platform: &str) -> Result<()> {
    strategy.validate()?;
    if platform == "ios" && strategy.rule_sets.iter().flat_map(|set| set.matchers.iter()).any(|m| matches!(m.kind.as_str(), "app" | "uid" | "geo-site" | "geo-ip")) {
        bail!("iOS 不支持应用、用户编号或地理数据库规则")
    }
    Ok(())
}
fn command(request: &str) -> Result<String> {
    let req: Value = serde_json::from_str(request).context("请求格式无效")?;
    let op = req.get("op").and_then(Value::as_str).unwrap_or("");
    if op == "init" { let platform = req.get("platform").and_then(Value::as_str).unwrap_or("android"); let mut guard = runtime().lock().unwrap(); *guard = Runtime::new(platform); return Ok(ok(guard.snapshot())); }
    let mut guard = runtime().lock().unwrap();
    match op {
        "snapshot" => Ok(ok(guard.snapshot())),
        "load" => { let text=req.get("document").and_then(Value::as_str).context("缺少 document")?; let loaded: Document=serde_json::from_str(text).context("配置文件格式无效")?; validate_mobile_strategy(&loaded.strategy,&guard.platform)?; loaded.catalog.validate()?; guard.strategy=loaded.strategy; guard.catalog=loaded.catalog; guard.revision+=1; Ok(ok(guard.snapshot())) },
        "export" => Ok(ok(Value::String(serde_json::to_string(&Document{strategy:guard.strategy.clone(),catalog:guard.catalog.clone()})?))),
        "connect" | "disconnect" | "probe" => bail!("native_required: VPN 连接由系统扩展负责"),
        "setMode" => { let mode=req.get("mode").and_then(Value::as_str).context("缺少 mode")?; guard.strategy.mixed_mode=myproxy_core::InboundMode::parse(mode)?; guard.revision+=1; Ok(ok(guard.snapshot())) },
        "setAutoConnect" => { guard.strategy.connect_on_launch=req.get("enabled").and_then(Value::as_bool).context("缺少 enabled")?; guard.revision+=1; Ok(ok(guard.snapshot())) },
        "setFallback" => { guard.strategy.unmatched_via=req.get("via").and_then(Value::as_str).context("缺少 via")?.into(); guard.revision+=1; Ok(ok(guard.snapshot())) },
        "select" => { let group=req.get("group").and_then(Value::as_str).context("缺少 group")?; let member=req.get("member").and_then(Value::as_str).unwrap_or(""); if group=="GLOBAL" { guard.strategy.global_selected=member.into(); } else if let Some(g)=guard.strategy.groups.iter_mut().find(|g|g.name==group || g.id==group) { g.selected=member.into(); } else { bail!("节点组不存在") } guard.revision+=1; Ok(ok(guard.snapshot())) },
        "import" => { let text=req.get("text").and_then(Value::as_str).context("缺少 text")?; let source_id=req.get("sourceId").and_then(Value::as_str); let source_name=req.get("name").and_then(Value::as_str).unwrap_or("导入节点"); let source_url=req.get("sourceURL").and_then(Value::as_str); let source=source_name; let values=myproxy_core::parse_links(text)?; let staged=values.into_iter().map(|value|parse_node(value,source,"导入节点")).collect::<Result<Vec<_>>>()?; let mut next=guard.catalog.clone(); if let Some(id)=source_id { if let Some(old)=guard.strategy.subscriptions.iter().find(|s|s.id==id) { next.nodes.retain(|n|n.subscription!=old.name); } } else { next.nodes.retain(|n|n.subscription!=source); } next.nodes.extend(staged); next.validate()?; if let Some(id)=source_id { if let Some(old)=guard.strategy.subscriptions.iter_mut().find(|s|s.id==id) { old.name=source.into(); if let Some(url)=source_url {old.url=url.into();} } } else if let Some(url)=source_url { guard.strategy.subscriptions.push(Subscription{id:uuid::Uuid::new_v4().to_string(),name:source.into(),url:url.into()}); } guard.catalog=next; guard.revision+=1; Ok(ok(guard.snapshot())) },
        "addSource" => { let name=req.get("name").and_then(Value::as_str).unwrap_or("订阅"); let url=req.get("url").and_then(Value::as_str).context("缺少 url")?; let sub=Subscription{id:uuid::Uuid::new_v4().to_string(),name:name.into(),url:url.into()}; guard.strategy.subscriptions.push(sub.clone()); guard.revision+=1; Ok(ok(json!({"sourceId":sub.id,"snapshot":guard.snapshot()}))) },
        "removeSource" => { let id=req.get("id").and_then(Value::as_str).context("缺少 id")?; guard.strategy.subscriptions.retain(|s|s.id!=id); let names=guard.strategy.subscriptions.iter().map(|s|s.name.clone()).collect::<std::collections::HashSet<_>>(); guard.catalog.nodes.retain(|n|names.contains(&n.subscription)); guard.revision+=1; Ok(ok(guard.snapshot())) },
        "saveRule" => { let name=req.get("name").and_then(Value::as_str).unwrap_or("新规则"); let kind=req.get("kind").and_then(Value::as_str).unwrap_or("domain"); let value=req.get("value").and_then(Value::as_str).unwrap_or(""); let via=req.get("via").and_then(Value::as_str).unwrap_or("节点选择"); let set=RuleSet{unavailable_fallback:None,id:req.get("id").and_then(Value::as_str).unwrap_or("").to_string(),name:name.into(),via:via.into(),matchers:vec![myproxy_core::Matcher{kind:kind.into(),value:value.into()}]}; if let Some(old)=guard.strategy.rule_sets.iter_mut().find(|r|!set.id.is_empty()&&r.id==set.id) {*old=set;} else { let mut set=set; if set.id.is_empty(){set.id=uuid::Uuid::new_v4().to_string();} guard.strategy.rule_sets.push(set); } guard.revision+=1; Ok(ok(guard.snapshot())) },
        "deleteRule" => { let id=req.get("id").and_then(Value::as_str).context("缺少 id")?; guard.strategy.rule_sets.retain(|r|r.id!=id); guard.revision+=1; Ok(ok(guard.snapshot())) },
        "health" => { let name=req.get("node").and_then(Value::as_str).context("缺少 node")?; let failed=req.get("failed").and_then(Value::as_bool).unwrap_or(false); let item=guard.health.entry(name.into()).or_default(); item.delay_ms=req.get("delayMs").and_then(Value::as_u64).map(|v|v as u32); if failed {item.failures+=1;} else {item.failures=0;} Ok(ok(guard.snapshot())) },
        "route" => { let host=req.get("host").and_then(Value::as_str).context("缺少 host")?; let hostname=req.get("hostname").and_then(Value::as_str); let port=req.get("port").and_then(Value::as_u64).unwrap_or(443) as u16; let network=req.get("network").and_then(Value::as_str).unwrap_or("tcp"); let context=myproxy_core::xray::policy::FlowContext{host,hostname,port,network,user_id:None,applications:&[]}; let decision=myproxy_core::xray::policy::decide_application(&guard.strategy,&guard.catalog,&guard.health,&context); if guard.platform=="ios" && matches!(decision.route,myproxy_core::Route::Direct) && !host.eq_ignore_ascii_case("localhost") { bail!("iOS 不支持运行时动态直连") } let (action,tag,node)=match decision.route {myproxy_core::Route::Direct=>("direct",None,None),myproxy_core::Route::Reject=>("reject",None,None),myproxy_core::Route::Node(n)=>("proxy",Some(stable_tag(&n)),Some(n))}; Ok(ok(json!({"action":action,"tag":tag,"node":node,"rule":decision.rule,"chain":decision.chain,"revision":guard.revision}))) },
        "render" => { let mut outbounds=Vec::new(); let mut nodes=Vec::new(); for node in guard.catalog.nodes.iter() {let tag=stable_tag(&node.name); if let Ok(outbound)=myproxy_core::render_node(node,&tag){outbounds.push(outbound);nodes.push(json!({"name":node.name,"tag":tag}));}} Ok(ok(json!({"config":json!({"outbounds":outbounds}).to_string(),"nodes":nodes,"revision":guard.revision}))) },
        _ => bail!("不支持的操作")
    }
}

fn stable_tag(name: &str) -> String { use std::hash::{Hash, Hasher}; let mut h=std::collections::hash_map::DefaultHasher::new(); name.hash(&mut h); format!("node-{:016x}",h.finish()) }
pub fn call_json(request: &str) -> String { std::panic::catch_unwind(|| command(request).unwrap_or_else(|e| err("invalid_config", e.to_string()))).unwrap_or_else(|_| err("internal_error", "核心处理失败")) }
#[no_mangle] pub extern "C" fn myproxy_mobile_call(request_json: *const c_char) -> *mut c_char { if request_json.is_null(){return CString::new(err("invalid_request","请求为空")).unwrap().into_raw()} let request=unsafe{CStr::from_ptr(request_json)}.to_string_lossy(); CString::new(call_json(&request)).unwrap().into_raw() }
#[no_mangle] pub extern "C" fn myproxy_mobile_free(response: *mut c_char) { if !response.is_null(){unsafe{drop(CString::from_raw(response));}} }

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_one_leaper_myproxy_core_NativeCore_call(mut env: jni::JNIEnv<'_>, _class: jni::objects::JClass<'_>, request: jni::objects::JString<'_>) -> jni::sys::jstring {
    let input = env.get_string(&request).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    env.new_string(call_json(&input)).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}
