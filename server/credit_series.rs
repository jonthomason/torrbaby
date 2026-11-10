//! Final credit segments from one file's scan plus its series' other episodes.
//! The scanner reports rolling-crawl segments and static credit-card candidates;
//! episodes of one title repeat the same crew and cast cards, so agreement across
//! already analysed episodes confirms a weaker candidate and exposes a one-off run
//! of on-screen text that looks nothing like the series' real credits.
use crate::storage::{Database, now};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashSet;

/// Candidates at or above this confidence stand on their own.
const STANDALONE:f64=0.6;
/// Weaker candidates need at least one agreeing episode.
const WITH_SERIES:f64=0.45;
/// Strong candidates are still rejected when the series has established credits
/// and this run shares nothing with any of them.
const ESTABLISHED_PEERS:usize=3;
const UNCONDITIONAL:f64=0.8;
/// Shared credited names that make two runs the same credits. Measured on the
/// library: episodes of one series share at least 5, unrelated series at most 3
/// (common post houses and vendors).
const SHARED_NAMES:usize=4;
/// Segments closer than this are one credits sequence (cards before a crawl).
const MERGE_GAP:f64=12.0;
/// A weak run this close to accepted credits belongs to them.
const ADJACENT_GAP:f64=45.0;

fn names(card:&Value)->HashSet<&str> {
    card["signature"].as_array().map(|a|a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
}
fn cards(document:&Value)->&[Value] {document["cards"].as_array().map(Vec::as_slice).unwrap_or(&[])}

/// The scanner's own `segments` are rolling-crawl detections; keep them apart so
/// resolution can be repeated as more episodes arrive.
pub fn prepare(mut document:Value)->Value {
    if document["state"]=="ready" && document.get("rolling_segments").is_none() {
        document["rolling_segments"]=document["segments"].take();
        if !document["rolling_segments"].is_array() {document["rolling_segments"]=json!([]);}
    }
    document
}

pub fn resolve(document:&Value, peers:&[Value])->Value {
    let mut out=document.clone();
    if document["state"]!="ready" {return out;}
    let established=peers.iter().filter(|p|!cards(p).is_empty()).count();
    let mut segments:Vec<Value>=document["rolling_segments"].as_array().cloned().unwrap_or_default();
    let mut candidates=Vec::new();
    for card in cards(document) {
        let (Some(start),Some(end))=(card["start"].as_f64(),card["end"].as_f64()) else {continue};
        let confidence=card["confidence"].as_f64().unwrap_or(0.0);
        let own=names(card);
        let agreeing=peers.iter().filter(|p|cards(p).iter().any(|c|own.intersection(&names(c)).count()>=SHARED_NAMES)).count();
        let verdict=if established>=ESTABLISHED_PEERS && agreeing==0 && confidence<UNCONDITIONAL {"unlike_series"}
            else if confidence>=STANDALONE || (confidence>=WITH_SERIES && agreeing>=1) {"accepted"}
            else {"insufficient"};
        candidates.push((card,start,end,confidence,agreeing,verdict));
    }
    // A short weak run right next to accepted credits (the director card before
    // a long crawl, a closing notice after it) is part of the same credits.
    loop {
        let accepted:Vec<(f64,f64)>=candidates.iter().filter(|c|matches!(c.5,"accepted"|"adjacent")).map(|c|(c.1,c.2)).collect();
        let mut changed=false;
        for c in candidates.iter_mut().filter(|c|c.5=="insufficient" && c.3>=WITH_SERIES) {
            if accepted.iter().any(|(s,e)|c.2+ADJACENT_GAP>=*s && c.1<=e+ADJACENT_GAP) {c.5="adjacent";changed=true;}
        }
        if !changed {break;}
    }
    let mut review=Vec::new();
    for (card,start,end,confidence,agreeing,verdict) in candidates {
        review.push(json!({"start":start,"verdict":verdict,"agreeing_episodes":agreeing}));
        if matches!(verdict,"accepted"|"adjacent") {
            let plain=card["plain_share"].as_f64().unwrap_or(1.0)>=0.5;
            segments.push(json!({"start":start,"end":end,"method":"cards-v1","background":if plain{"plain"}else{"picture"},
                "confidence":((confidence+0.04*agreeing.min(3) as f64).min(0.99)*100.0).round()/100.0}));
        }
    }
    out["segments"]=Value::Array(merge(segments));
    out["series_review"]=Value::Array(review);
    out
}

fn merge(mut segments:Vec<Value>)->Vec<Value> {
    segments.retain(|s|s["start"].as_f64().zip(s["end"].as_f64()).is_some_and(|(a,b)|a.is_finite()&&b>=a));
    segments.sort_by(|a,b|a["start"].as_f64().unwrap_or(0.0).total_cmp(&b["start"].as_f64().unwrap_or(0.0)));
    let mut out:Vec<Value>=Vec::new();
    for s in segments {
        if let Some(last)=out.last_mut() {
            let (end,start)=(last["end"].as_f64().unwrap_or(0.0),s["start"].as_f64().unwrap_or(0.0));
            if start<=end+MERGE_GAP {
                last["end"]=json!(end.max(s["end"].as_f64().unwrap_or(end)));
                let mut methods:Vec<&str>=last["method"].as_str().unwrap_or("").split('+').chain(s["method"].as_str()).filter(|m|!m.is_empty()).collect();
                methods.sort_unstable();methods.dedup();
                let method=methods.join("+");
                last["method"]=json!(method);
                let confidence=last["confidence"].as_f64().unwrap_or(0.0).max(s["confidence"].as_f64().unwrap_or(0.0));
                last["confidence"]=json!(confidence);
                if s["background"]=="plain" {last["background"]=json!("plain");}
                continue;
            }
        }
        out.push(s);
    }
    out
}

/// Re-resolve every current analysis of one title so a newly analysed episode
/// can confirm (or contradict) the ones before it. Returns how many changed.
pub fn resolve_title(db:&Database, entity:&str, version:i64)->Result<usize> {
    let rows=db.rows("SELECT a.id AS id,c.document AS analysis FROM library_assets a JOIN credits_analysis c ON a.id=c.asset_id WHERE json_extract(a.document,'$.entity_id')=? AND c.version=?",rusqlite::params![entity,version])?;
    let documents:Vec<(String,Value)>=rows.iter().filter_map(|r|Some((r["id"].as_str()?.to_owned(),serde_json::from_str(r["analysis"].as_str()?).ok()?))).collect();
    let mut changed=0;
    for (i,(id,document)) in documents.iter().enumerate() {
        if document["state"]!="ready" {continue;}
        let peers:Vec<Value>=documents.iter().enumerate().filter(|(j,_)|*j!=i).map(|(_,(_,d))|d.clone()).filter(|d|d["state"]=="ready").collect();
        let resolved=resolve(document,&peers);
        if resolved!=*document {
            db.connection.execute("UPDATE credits_analysis SET document=?,updated=? WHERE asset_id=? AND version=?",rusqlite::params![resolved.to_string(),now(),id,version])?;
            changed+=1;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn card(start:f64,confidence:f64,names:&[&str])->Value {json!({"start":start,"end":start+60.0,"method":"cards-v1","confidence":confidence,"plain_share":1.0,"signature":names})}
    fn scan(cards:Vec<Value>,rolling:Value)->Value {prepare(json!({"state":"ready","duration":3000.0,"segments":rolling,"cards":cards}))}
    const CREW:[&str;6]=["michaelwilson","nickheckstallsmith","judithmilne","ronalddmoore","marildavis","terrydresbach"];

    #[test]
    fn confident_cards_stand_alone_and_merge_with_a_following_crawl() {
        let doc=scan(vec![card(2900.0,0.95,&CREW)],json!([{"start":2965.0,"end":2990.0,"method":"rolling-text-v1","confidence":0.9}]));
        let out=resolve(&doc,&[]);
        assert_eq!(out["segments"],json!([{"start":2900.0,"end":2990.0,"method":"cards-v1+rolling-text-v1","background":"plain","confidence":0.95}]));
        // Re-resolution is idempotent: the scanner's crawl is kept apart.
        assert_eq!(resolve(&out,&[]),out);
    }
    #[test]
    fn weak_cards_need_an_agreeing_episode() {
        let doc=scan(vec![card(2950.0,0.5,&CREW)],json!([]));
        assert_eq!(resolve(&doc,&[])["segments"],json!([]));
        // ...or confident credits right next to them (a director card before the crawl).
        let film=scan(vec![card(2800.0,0.55,&["a1234"]),card(2900.0,0.9,&["b1234"]),card(2500.0,0.55,&["c1234"])],json!([]));
        let out=resolve(&film,&[]);
        assert_eq!(out["segments"].as_array().map(Vec::len),Some(2));
        assert_eq!(out["segments"][0]["start"],2800.0);
        assert_eq!(out["series_review"][2]["verdict"],"insufficient");
        let peer=scan(vec![card(2800.0,0.95,&CREW[..5])],json!([]));
        let out=resolve(&doc,&[peer]);
        assert_eq!(out["segments"][0]["start"],2950.0);
        assert_eq!(out["segments"][0]["confidence"],0.54);
        // Unrelated series sharing only vendors do not agree.
        let other=scan(vec![card(2800.0,0.95,&["michaelwilson","panavision","harborpicturecompany","x"])],json!([]));
        assert_eq!(resolve(&doc,&[other])["segments"],json!([]));
    }
    #[test]
    fn a_run_unlike_an_established_series_is_rejected() {
        let peers:Vec<Value>=(0..3).map(|i|scan(vec![card(2800.0+i as f64,0.95,&CREW)],json!([]))).collect();
        let odd=scan(vec![card(1000.0,0.7,&["exit","southgeorgia","keepout","floorthree"])],json!([]));
        let out=resolve(&odd,&peers);
        assert_eq!(out["segments"],json!([]));
        assert_eq!(out["series_review"][0]["verdict"],"unlike_series");
        // Overwhelming evidence still stands on its own.
        let strong=scan(vec![card(1000.0,0.95,&["a1234","b1234","c1234","d1234"])],json!([]));
        assert_eq!(resolve(&strong,&peers)["segments"][0]["start"],1000.0);
    }
    #[test]
    fn title_resolution_updates_earlier_episodes() -> Result<()> {
        let db=Database{connection:rusqlite::Connection::open_in_memory()?};
        db.connection.execute_batch(include_str!("schema.sql"))?;
        for (id,confidence) in [("asset-a",0.5),("asset-b",0.95)] {
            db.connection.execute("INSERT INTO library_assets(id,document) VALUES(?,?)",rusqlite::params![id,json!({"entity_id":"tmdb:tv:1","season":1}).to_string()])?;
            let doc=scan(vec![card(2900.0,confidence,&CREW)],json!([]));
            db.connection.execute("INSERT INTO credits_analysis VALUES(?,?,?,?,?)",rusqlite::params![id,"r",2,doc.to_string(),0.0])?;
        }
        assert_eq!(resolve_title(&db,"tmdb:tv:1",2)?,2);
        let a:String=db.connection.query_row("SELECT document FROM credits_analysis WHERE asset_id='asset-a'",[],|r|r.get(0))?;
        assert_eq!(serde_json::from_str::<Value>(&a)?["segments"][0]["start"],2900.0);
        assert_eq!(resolve_title(&db,"tmdb:tv:1",2)?,0);
        Ok(())
    }
}
