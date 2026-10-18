use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use maxminddb::{Reader, geoip2};
use tracing::info;

pub struct Geo {
    reader: Reader<Vec<u8>>,
}

impl Geo {
    pub fn load(path: &Path) -> Self {
        let reader = Reader::open_readfile(path).unwrap_or_else(|error| {
            panic!("cannot open geoip database {}: {error}", path.display())
        });
        let geo = Self { reader };
        geo.country(Ipv4Addr::new(1, 1, 1, 1));
        info!(path = %path.display(), "geoip database loaded");
        geo
    }

    pub fn country(&self, ip: Ipv4Addr) -> Option<String> {
        let result = self
            .reader
            .lookup(IpAddr::V4(ip))
            .expect("geoip lookup failed");
        let record: Option<geoip2::Country> = result.decode().expect("geoip decode failed");
        record.and_then(|record| record.country.iso_code.map(str::to_owned))
    }
}
