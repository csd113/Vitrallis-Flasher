//! ureq's pinned transport extension supplies per-I/O cancellation and idle deadlines.
use crate::Cancellation;
use ureq::unversioned::{
    resolver::DefaultResolver,
    transport::{
        Buffers, ConnectionDetails, Connector, NextTimeout, RustlsConnector, TcpConnector,
        Transport, time::Duration,
    },
};
#[derive(Debug)]
struct PublicNetwork;
impl Connector for PublicNetwork {
    type Out = ();
    fn connect(
        &self,
        details: &ConnectionDetails,
        chained: Option<()>,
    ) -> Result<Option<()>, ureq::Error> {
        if details.addrs.is_empty() || details.addrs.iter().any(|address| !public_ip(address.ip()))
        {
            return Err(ureq::Error::ConnectionFailed);
        }
        Ok(chained)
    }
}
fn public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192 && b == 0)
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        std::net::IpAddr::V6(ip) => {
            let s = ip.segments();
            s[0] & 0xe000 == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
        }
    }
}
#[derive(Debug)]
struct BoundedConnector(Cancellation);
#[derive(Debug)]
struct Bounded<T> {
    inner: T,
    cancel: Cancellation,
}
impl<T: Transport> Connector<T> for BoundedConnector {
    type Out = Bounded<T>;
    fn connect(
        &self,
        _: &ConnectionDetails,
        chained: Option<T>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(chained.map(|inner| Bounded {
            inner,
            cancel: self.0.clone(),
        }))
    }
}
fn deadline(mut timeout: NextTimeout) -> NextTimeout {
    timeout.after = timeout.after.min(Duration::from_secs(5));
    timeout
}
impl<T: Transport> Bounded<T> {
    fn check(&self) -> Result<(), ureq::Error> {
        self.cancel.check().map_err(|_| {
            ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ))
        })
    }
}
impl<T: Transport> Transport for Bounded<T> {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }
    fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
        self.check()?;
        self.inner.transmit_output(amount, deadline(timeout))
    }
    fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
        self.check()?;
        self.inner.await_input(deadline(timeout))
    }
    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }
    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}
pub fn agent(cancel: &Cancellation) -> ureq::Agent {
    use std::time::Duration;
    let config = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_mins(30)))
        .timeout_resolve(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_send_request(Some(Duration::from_secs(5)))
        .timeout_recv_response(Some(Duration::from_secs(10)))
        .proxy(None)
        .build();
    let connector =
        ().chain(PublicNetwork)
            .chain(TcpConnector::default())
            .chain(BoundedConnector(cancel.clone()))
            .chain(RustlsConnector::default());
    ureq::Agent::with_parts(config, connector, DefaultResolver::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserved_networks_are_rejected_before_connection() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "192.168.1.1",
            "100.64.0.1",
            "198.18.0.1",
            "::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_ip("140.82.112.3".parse().unwrap()));
    }
    #[test]
    fn io_deadline_preserves_shorter_global_deadline() {
        let t = NextTimeout {
            after: Duration::from_secs(1),
            reason: ureq::Timeout::Global,
        };
        assert_eq!(deadline(t).after, Duration::from_secs(1));
        let t = NextTimeout {
            after: Duration::from_secs(500),
            reason: ureq::Timeout::Global,
        };
        assert_eq!(deadline(t).after, Duration::from_secs(5));
    }
}
