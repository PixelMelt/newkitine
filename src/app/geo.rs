use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use maxminddb::{Reader, geoip2};

pub struct Geo {
    reader: Reader<Vec<u8>>,
}

impl Geo {
    pub fn load(path: &Path) -> Self {
        let reader = Reader::open_readfile(path).unwrap_or_else(|error| {
            panic!("cannot open geoip database {}: {error}", path.display())
        });
        Self { reader }
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
