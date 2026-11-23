//! What `ipconfig()` shows: each address with its gateway and adapter,
//! `192.168.1.23, gateway 192.168.1.1 (Wi-Fi)`.

/// An address this computer has on a network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkAddress {
    /// The adapter's name, as the system shows it: Wi-Fi, Ethernet, wlan0.
    pub adapter: String,
    /// 192.168.1.23
    pub address: String,
    /// Where its traffic leaves the network: 192.168.1.1; `None` for none.
    pub gateway: Option<String>,
}

/// Reads the computer's addresses (tests use fixed ones).
pub trait NetworkReader {
    /// The addresses of the adapters that are connected; none when there's no network.
    fn read(&self) -> Vec<NetworkAddress>;
}

pub const NO_NETWORK: &str = "No network connection";

/// Only the addresses with a gateway, which reach beyond the computer, when there are any: the
/// others are usually virtual adapters, such as WSL's, which would hide the one wanted. Several
/// are separated by "; ".
pub fn config_text(addresses: &[NetworkAddress]) -> String {
    let with_gateway: Vec<&NetworkAddress> =
        addresses.iter().filter(|a| a.gateway.is_some()).collect();
    let shown = if with_gateway.is_empty() { addresses.iter().collect() } else { with_gateway };

    if shown.is_empty() {
        return NO_NETWORK.to_string();
    }

    shown
        .iter()
        .map(|a| match &a.gateway {
            Some(gateway) => format!("{}, gateway {gateway} ({})", a.address, a.adapter),
            None => format!("{} ({})", a.address, a.adapter),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The addresses the system's adapters have now: IPv4 ones, or else IPv6 ones that aren't only
/// for the adapter's own link. Adapters that are off, the loopback and tunnels are left out.
pub struct SystemNetwork;

impl NetworkReader for SystemNetwork {
    fn read(&self) -> Vec<NetworkAddress> {
        use netdev::interface::state::OperState;
        use netdev::interface::types::InterfaceType;

        let mut addresses = Vec::new();
        for adapter in netdev::get_interfaces() {
            if !adapter.is_up()
                || adapter.oper_state == OperState::Down
                || adapter.is_loopback()
                || adapter.if_type == InterfaceType::Tunnel
            {
                continue;
            }

            let gateway = adapter
                .gateway
                .as_ref()
                .and_then(|gateway| gateway.ipv4.iter().find(|ip| !ip.is_unspecified()))
                .map(|ip| ip.to_string());

            let mut shown: Vec<String> = adapter.ipv4.iter().map(|net| net.addr().to_string()).collect();
            if shown.is_empty() {
                shown = adapter
                    .ipv6
                    .iter()
                    .map(|net| net.addr())
                    .filter(|ip| ip.segments()[0] & 0xffc0 != 0xfe80) // not link-local
                    .map(|ip| ip.to_string())
                    .collect();
            }

            let name = adapter.friendly_name.clone().unwrap_or_else(|| adapter.name.clone());
            addresses.extend(shown.into_iter().map(|address| NetworkAddress {
                adapter: name.clone(),
                address,
                gateway: gateway.clone(),
            }));
        }
        addresses
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calculator::engine::testing::{FakeNetwork, calculator_with_network};

    fn wifi() -> NetworkAddress {
        address("Wi-Fi", "192.168.1.23", Some("192.168.1.1"))
    }

    fn address(adapter: &str, address: &str, gateway: Option<&str>) -> NetworkAddress {
        NetworkAddress {
            adapter: adapter.into(),
            address: address.into(),
            gateway: gateway.map(Into::into),
        }
    }

    fn ipconfig(addresses: Vec<NetworkAddress>, line: &str) -> crate::calculator::engine::Result<String> {
        calculator_with_network(FakeNetwork(addresses))
            .calculate(line, &Default::default(), true)
            .map(|calculation| calculation.text)
    }

    #[test]
    fn is_the_address_and_gateway_of_the_adapter() {
        let calculation = calculator_with_network(FakeNetwork(vec![wifi()]))
            .calculate("IPCONFIG()", &Default::default(), true)
            .unwrap();
        assert_eq!(calculation.text, "192.168.1.23, gateway 192.168.1.1 (Wi-Fi)");
        assert_eq!(
            calculation.value,
            Some(crate::calculator::engine::Value::Text(calculation.text.clone()))
        );
    }

    #[test]
    fn shows_only_the_adapters_with_a_gateway_when_there_are_any() {
        let addresses = vec![
            address("vEthernet (WSL)", "172.20.0.1", None),
            address("Ethernet", "10.0.0.5", Some("10.0.0.1")),
            wifi(),
        ];
        assert_eq!(
            ipconfig(addresses, "ipconfig()").unwrap(),
            "10.0.0.5, gateway 10.0.0.1 (Ethernet); 192.168.1.23, gateway 192.168.1.1 (Wi-Fi)"
        );
    }

    #[test]
    fn without_a_gateway_shows_what_there_is() {
        assert_eq!(ipconfig(vec![], "ipconfig()").unwrap(), NO_NETWORK);
        assert_eq!(
            ipconfig(vec![address("Ethernet", "169.254.10.2", None)], "ipconfig()").unwrap(),
            "169.254.10.2 (Ethernet)"
        );
    }

    #[test]
    fn is_a_text_that_cant_be_calculated_with() {
        for (line, message) in [
            ("ipconfig(1)", "Function 'ipconfig' expects no arguments"),
            ("ipconfig() + 1", "Can't add a text and a number"),
            ("today + ipconfig()", "Can't add a date and a text"),
            ("sqrt(ipconfig())", "Expected a number, not a text"),
            ("-ipconfig()", "A text can't be negative"),
            ("ipconfig = 2", "'ipconfig' is a function, so it can't be given a value"),
        ] {
            assert_eq!(ipconfig(vec![wifi()], line).unwrap_err().message, message, "{line}");
        }
    }

    #[test]
    fn the_system_reader_never_gives_the_loopback() {
        let addresses = SystemNetwork.read();
        assert!(addresses.iter().all(|a| a.address != "127.0.0.1" && a.address != "::1"));
    }
}
