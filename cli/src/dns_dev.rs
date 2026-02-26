use std::{
    fs,
    net::SocketAddr,
    path::PathBuf,
    str::FromStr,
    time::{Duration, SystemTime},
};

use hickory_proto::rr::{
    DNSClass, Name, RData, Record,
    rdata::{NS, SOA, TXT},
};
use hickory_server::{
    ServerFuture,
    authority::{Catalog, ZoneType},
    store::in_memory::InMemoryAuthority,
};
use iroh_base::EndpointId;
use n0_error::StdResultExt;
use serde::{Deserialize, Serialize};
use tokio::{net::UdpSocket, sync::RwLock, time};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsDevConfig {
    pub origin: String,
    #[serde(default)]
    pub records: Vec<DnsDevRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsDevRecord {
    pub endpoint_id: String,
    #[serde(default)]
    pub relay: Option<String>,
    #[serde(default)]
    pub addrs: Vec<String>,
}

pub async fn serve(
    bind_addr: SocketAddr,
    config_path: PathBuf,
    origin: String,
    reload_interval: Duration,
) -> n0_error::Result<()> {
    let mut last_modified = fs::metadata(&config_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let catalog = ArcCatalog::new(build_catalog(&config_path, &origin)?);
    let handler = SharedCatalog::new(catalog.clone());

    let mut server = ServerFuture::new(handler);
    let socket = UdpSocket::bind(bind_addr).await?;
    server.register_socket(socket);

    let reload_task = tokio::spawn(async move {
        let mut interval = time::interval(reload_interval);
        loop {
            interval.tick().await;
            if let Ok(modified) = fs::metadata(&config_path).and_then(|m| m.modified())
                && modified > last_modified
            {
                match build_catalog(&config_path, &origin) {
                    Ok(new_catalog) => {
                        catalog.replace(new_catalog).await;
                        last_modified = modified;
                    }
                    Err(err) => {
                        warn!("failed to reload dns dev config: {err:#}");
                    }
                }
            }
        }
    });

    info!(?bind_addr, "dns-dev server started");
    server.block_until_done().await.anyerr()?;
    reload_task.abort();
    Ok(())
}

pub fn upsert(
    config_path: PathBuf,
    origin: String,
    endpoint_id: String,
    relay: Option<String>,
    addrs: Vec<String>,
) -> n0_error::Result<()> {
    let mut config = if config_path.exists() {
        serde_yml::from_str::<DnsDevConfig>(&fs::read_to_string(&config_path)?).anyerr()?
    } else {
        DnsDevConfig {
            origin: origin.clone(),
            records: Vec::new(),
        }
    };

    if config.origin.is_empty() || config.origin != origin {
        config.origin = origin;
    }

    if let Some(record) = config
        .records
        .iter_mut()
        .find(|r| r.endpoint_id == endpoint_id)
    {
        if relay.is_some() {
            record.relay = relay;
        }
        if !addrs.is_empty() {
            record.addrs = addrs;
        }
    } else {
        config.records.push(DnsDevRecord {
            endpoint_id,
            relay,
            addrs,
        });
    }

    let data = serde_yml::to_string(&config).anyerr()?;
    fs::write(config_path, data)?;
    Ok(())
}

fn build_catalog(config_path: &PathBuf, fallback_origin: &str) -> n0_error::Result<Catalog> {
    let config = if config_path.exists() {
        serde_yml::from_str::<DnsDevConfig>(&fs::read_to_string(config_path)?).anyerr()?
    } else {
        DnsDevConfig {
            origin: fallback_origin.to_string(),
            records: Vec::new(),
        }
    };

    let origin = if config.origin.is_empty() {
        fallback_origin.to_string()
    } else {
        config.origin.clone()
    };
    let origin = normalize_origin(&origin);

    let zone_name = Name::from_str(&format!("{origin}.")).anyerr()?;
    let mut authority = InMemoryAuthority::empty(zone_name.clone(), ZoneType::Primary, false);

    let serial = 1;
    let ttl = 30;
    let mname = Name::from_str(&format!("ns.{origin}.")).anyerr()?;
    let rname = Name::from_str(&format!("admin.{origin}.")).anyerr()?;
    let soa = SOA::new(mname.clone(), rname, serial, 60, 60, 60, 30);
    let mut soa_record = Record::from_rdata(zone_name.clone(), ttl, RData::SOA(soa));
    soa_record.set_dns_class(DNSClass::IN);
    authority.upsert_mut(soa_record, serial);

    let mut ns_record = Record::from_rdata(zone_name.clone(), ttl, RData::NS(NS(mname)));
    ns_record.set_dns_class(DNSClass::IN);
    authority.upsert_mut(ns_record, serial);

    for record in config.records {
        let endpoint_id = EndpointId::from_str(&record.endpoint_id)?;
        let z32_id = z32::encode(endpoint_id.as_bytes());
        let name = Name::from_str(&format!("_iroh.{z32_id}.{origin}.")).anyerr()?;

        let mut txt_entries = Vec::new();
        if let Some(relay) = record.relay {
            txt_entries.push(format!("relay={relay}"));
        }
        if !record.addrs.is_empty() {
            txt_entries.push(format!("addr={}", record.addrs.join(" ")));
        }
        if txt_entries.is_empty() {
            continue;
        }
        let txt = TXT::new(txt_entries);
        let mut txt_record = Record::from_rdata(name, ttl, RData::TXT(txt));
        txt_record.set_dns_class(DNSClass::IN);
        authority.upsert_mut(txt_record, serial);
    }

    let mut catalog = Catalog::new();
    catalog.upsert(zone_name.into(), vec![std::sync::Arc::new(authority)]);
    Ok(catalog)
}

fn normalize_origin(origin: &str) -> String {
    origin.trim_end_matches('.').to_string()
}

#[derive(Clone)]
struct ArcCatalog {
    inner: std::sync::Arc<RwLock<Catalog>>,
}

impl ArcCatalog {
    fn new(catalog: Catalog) -> Self {
        Self {
            inner: std::sync::Arc::new(RwLock::new(catalog)),
        }
    }

    async fn replace(&self, catalog: Catalog) {
        let mut inner = self.inner.write().await;
        *inner = catalog;
        info!("dns-dev catalog reloaded");
    }
}

#[derive(Clone)]
struct SharedCatalog {
    inner: ArcCatalog,
}

impl SharedCatalog {
    fn new(inner: ArcCatalog) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl hickory_server::server::RequestHandler for SharedCatalog {
    async fn handle_request<R: hickory_server::server::ResponseHandler>(
        &self,
        request: &hickory_server::server::Request,
        response_handle: R,
    ) -> hickory_server::server::ResponseInfo {
        let catalog = self.inner.inner.read().await;
        hickory_server::server::RequestHandler::handle_request(&*catalog, request, response_handle)
            .await
    }
}
