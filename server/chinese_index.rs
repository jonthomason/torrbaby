//! Public 1LOU metadata. Attachments are fetched only on opening a listing.
use super::*;
use sha1::{Digest, Sha1};

pub(super) fn listings(raw: &str) -> Result<Vec<Item>> {
    let root: Value = serde_json::from_str(raw)?;
    ensure!(root["ok"] == true, "Search unavailable");
    let hits = root["data"]["hits"].as_array().context("Invalid search response")?;
    Ok(hits.iter().take(100).filter_map(|hit| {
        // Only video forums; exclude cloud-storage-only posts, games and books.
        if ![1,2,3,4,10,17].contains(&hit["fid"].as_u64()?) || hit["files"].as_u64()? == 0 { return None; }
        let id = hit["tid"].as_u64()?.to_string();
        let name = hit["subject"].as_str()?.to_owned();
        if name.is_empty() || name.len()>4096 { return None; }
        let mut row = item(name, String::new(), Source {provider:"1lou".into(),url:format!("http://www.1lou.me/thread-{id}.htm"),id});
        static SIZE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)([0-9]+(?:\.[0-9]+)?)\s*(GiB|GB|G|MiB|MB|M|TiB|TB|T)(?:[\] /]|$)").unwrap());
        if let Some(size)=SIZE.captures(&row.name) {
            row.size_label=Some(format!("{} {}",&size[1],size[2].to_ascii_uppercase()));
        }
        row.magnet.clear(); // Never invent a torrent hash for a forum listing.
        row.added = hit["create_date"].as_u64().map(|n|n.to_string());
        Some(row)
    }).collect())
}
pub(super) fn thread(raw: &str) -> Result<(String, Vec<String>)> {
    let dom=tl::parse(raw,tl::ParserOptions::default())?;
    let p=dom.parser();
    let description=dom.query_selector(".message").and_then(|mut q|q.next()).and_then(|h|h.get(p))
        .map(|n|html_text(&n.inner_text(p))).unwrap_or_default();
    let re=Regex::new(r"^attach-download-([0-9]{1,12})\.htm$")?;
    let attachments=dom.query_selector("a").into_iter().flatten().filter_map(|h| {
        let tag=h.get(p)?.as_tag()?;
        if !tag.inner_text(p).trim().ends_with(".torrent") { return None; }
        let href=tag.attributes().get("href")??.as_utf8_str();
        Some(re.captures(&href)?[1].to_owned())
    }).collect();
    Ok((description.chars().take(20000).collect(),attachments))
}

// Bounded parser retains the original info dictionary bytes for the v1 hash.
#[derive(Debug)]
enum B { Bytes(Vec<u8>), Int(u64), List(Vec<B>), Dict(HashMap<Vec<u8>,B>) }
impl B {
    fn get(&self,key:&str)->Option<&B>{if let B::Dict(d)=self{d.get(key.as_bytes())}else{None}}
    fn text(&self)->Option<String>{if let B::Bytes(b)=self{Some(String::from_utf8_lossy(b).into_owned())}else{None}}
    fn number(&self)->Option<u64>{if let B::Int(n)=self{Some(*n)}else{None}}
}
struct Parser<'a>{ bytes:&'a[u8], at:usize, nodes:usize, info:Option<(usize,usize)> }
impl Parser<'_> {
    fn value(&mut self, depth:usize)->Result<B>{
        ensure!(depth<=32 && self.nodes<100000,"Torrent metadata too complex"); self.nodes+=1;
        let tag=*self.bytes.get(self.at).context("Truncated torrent")?;self.at+=1;
        match tag {
            b'i'=>{let start=self.at;while self.bytes.get(self.at).is_some_and(|c|*c!=b'e'){self.at+=1;}
                let n=std::str::from_utf8(&self.bytes[start..self.at])?.parse()?;
                ensure!(self.bytes.get(self.at)==Some(&b'e'),"Truncated integer");self.at+=1;Ok(B::Int(n))},
            b'l'=>{let mut out=Vec::new();while self.bytes.get(self.at)!=Some(&b'e'){out.push(self.value(depth+1)?);}
                self.at+=1;Ok(B::List(out))},
            b'd'=>{let mut out=HashMap::new();while self.bytes.get(self.at)!=Some(&b'e'){
                let B::Bytes(key)=self.value(depth+1)? else {anyhow::bail!("Invalid dictionary key")};
                let start=self.at;let value=self.value(depth+1)?;
                if depth==0 && key==b"info" {self.info=Some((start,self.at));}
                ensure!(out.insert(key,value).is_none(),"Duplicate dictionary key");
                }self.at+=1;Ok(B::Dict(out))},
            b'0'..=b'9'=>{let start=self.at-1;while self.bytes.get(self.at).is_some_and(|c|c.is_ascii_digit()){self.at+=1;}
                ensure!(self.bytes.get(self.at)==Some(&b':'),"Invalid byte string");
                let len:usize=std::str::from_utf8(&self.bytes[start..self.at])?.parse()?;self.at+=1;
                let end=self.at.checked_add(len).context("String overflow")?;
                let value=self.bytes.get(self.at..end).context("Truncated byte string")?.to_vec();self.at=end;Ok(B::Bytes(value))},
            _=>anyhow::bail!("Not a torrent attachment")
        }
    }
}
pub(super) fn torrent(bytes:&[u8],id:&str,url:&str)->Result<Value>{
    ensure!(bytes.len()<=4_194_304,"Torrent attachment too large");
    let mut parser=Parser{bytes,at:0,nodes:0,info:None};let root=parser.value(0)?;
    ensure!(parser.at==bytes.len(),"Trailing torrent data");
    let (start,end)=parser.info.context("Missing torrent info")?;
    let info=root.get("info").context("Missing torrent info")?;
    ensure!(info.get("pieces").is_some(),"Only v1-compatible torrents are supported");
    let hash=Sha1::digest(&bytes[start..end]).iter().map(|b|format!("{b:02x}")).collect::<String>();
    let name=info.get("name.utf-8").or_else(||info.get("name")).and_then(B::text).context("Missing torrent name")?;
    let mut files=Vec::new();let mut total=0u64;
    if let Some(B::List(entries))=info.get("files") {
        ensure!(entries.len()<=10000,"Too many files");
        for file in entries {
            let size=file.get("length").and_then(B::number).context("Invalid file length")?;
            let Some(B::List(parts))=file.get("path.utf-8").or_else(||file.get("path")) else {anyhow::bail!("Missing path")};
            let path=parts.iter().map(|v|v.text().context("Invalid path")).collect::<Result<Vec<_>>>()?.join("/");
            total=total.checked_add(size).context("Size overflow")?;
            files.push(json!({"name":path,"size_bytes":size,"hints":hints(&path)}));
        }
    }else{total=info.get("length").and_then(B::number).context("Invalid file length")?;
        files.push(json!({"name":name,"size_bytes":total,"hints":hints(&name)}));}
    let mut row=item(name,hash,Source{provider:"1lou".into(),id:id.into(),url:url.into()});row.size_bytes=Some(total);
    let mut magnet=reqwest::Url::parse(&row.magnet)?;
    if let Some(tracker)=root.get("announce").and_then(B::text){magnet.query_pairs_mut().append_pair("tr",&tracker);}
    row.magnet=magnet.to_string();
    Ok(json!({"item":row,"files":files}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_torrent_parser_rejects_non_torrents() {
        assert!(torrent(b"<html>login</html>","1","http://www.1lou.me/thread-1.htm").is_err());
        let mut raw = b"d4:infod6:lengthi123e4:name8:test.mp46:pieces20:".to_vec();
        raw.extend_from_slice(&[0;20]);raw.extend_from_slice(b"ee");
        let release=torrent(&raw,"1","http://www.1lou.me/thread-1.htm").unwrap();
        assert_eq!(release["item"]["size_bytes"],123);
        assert_eq!(release["files"][0]["name"],"test.mp4");
        assert_eq!(release["item"]["info_hash"].as_str().unwrap().len(),40);
        raw.push(b'e');assert!(torrent(&raw,"1","").is_err());
        assert!(torrent(&vec![b'l';100],"1","").is_err());
    }
    #[test]
    #[ignore = "Requires locally fetched public index fixtures"]
    fn live_public_listing_fixture() {
        let dir=std::env::var("TORRBABY_INDEX_FIXTURES").unwrap();
        let search=std::fs::read_to_string(format!("{dir}/1lou-results.json")).unwrap();
        let rows=listings(&search).unwrap();assert!(!rows.is_empty());assert!(rows.iter().all(|r|r.magnet.is_empty()));
        let html=std::fs::read_to_string(format!("{dir}/1lou-detail.html")).unwrap();
        let (_,attachments)=thread(&html).unwrap();assert!(attachments.contains(&"13093".into()));
        let raw=std::fs::read(format!("{dir}/1lou-attachment")).unwrap();
        let release=torrent(&raw,"10231","http://www.1lou.me/thread-10231.htm").unwrap();
        assert!(release["files"].as_array().unwrap().len()>=30);
        assert_eq!(release["item"]["info_hash"], "e303febfe03f53e5edd772500436db72884640e0");
        println!("hash={} bytes={} files={}",release["item"]["info_hash"],release["item"]["size_bytes"],release["files"].as_array().unwrap().len());
    }
}
