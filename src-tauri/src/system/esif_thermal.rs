// OpenRevo - Intel Dynamic Tuning (ESIF) CPU temperature reader
// Some firmwares report a fixed value through ACPI thermal zones. When Intel DTT is installed, its
// processor participant exposes live sensors through root\wmi:EsifDeviceInformation (requires elevation).

use windows::core::{w, BSTR, VARIANT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    EOAC_NONE, RPC_C_AUTHN_LEVEL_PKT_PRIVACY, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Wmi::{
    IWbemLocator, IWbemServices, WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
};

const RPC_C_AUTHN_WINNT: u32 = 10;
const RPC_C_AUTHZ_NONE: u32 = 0;
const MAX_CONSECUTIVE_FAILURES: u32 = 5;

/// The participant's domain 0 is the processor package; other domains report secondary sensors.
const PACKAGE_INSTANCE_SUFFIX: &str = "_0";

/// Must be created and used on one thread because it holds a COM interface.
pub struct EsifThermalReader {
    services: Option<IWbemServices>,
    consecutive_failures: u32,
    com_initialized: bool,
}

impl EsifThermalReader {
    pub fn new() -> Self {
        Self {
            services: None,
            consecutive_failures: 0,
            com_initialized: false,
        }
    }

    /// Returns the package temperature in Celsius. After repeated failures, such as on machines
    /// without Intel DTT, it stops querying so callers fall back without extra WMI traffic.
    pub fn read_cpu_temp(&mut self) -> Option<i32> {
        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            return None;
        }

        let temp = self.query_samples().and_then(|samples| select_package_temp(&samples));
        if temp.is_some() {
            self.consecutive_failures = 0;
        } else {
            self.consecutive_failures += 1;
            if self.consecutive_failures == MAX_CONSECUTIVE_FAILURES {
                crate::log_info!("ESIF_THERMAL", "Intel DTT temperature unavailable; using ACPI thermal zones.");
            }
        }
        temp
    }

    fn connect(&mut self) -> Option<()> {
        if self.services.is_some() {
            return Some(());
        }
        unsafe {
            if !self.com_initialized {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                self.com_initialized = true;
            }
            let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
            let services = locator
                .ConnectServer(
                    &BSTR::from("root\\wmi"),
                    &BSTR::new(),
                    &BSTR::new(),
                    &BSTR::new(),
                    0,
                    &BSTR::new(),
                    None,
                )
                .ok()?;
            let _ = CoSetProxyBlanket(
                &services,
                RPC_C_AUTHN_WINNT,
                RPC_C_AUTHZ_NONE,
                None,
                RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            );
            self.services = Some(services);
        }
        Some(())
    }

    fn query_samples(&mut self) -> Option<Vec<(String, i32)>> {
        self.connect()?;
        let services = self.services.as_ref()?;
        let mut samples = Vec::new();

        unsafe {
            let enumerator = match services.ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from("SELECT InstanceName, Temperature FROM EsifDeviceInformation"),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None,
            ) {
                Ok(enumerator) => enumerator,
                Err(_) => {
                    self.services = None;
                    return None;
                }
            };

            let mut objects = [None];
            let mut returned = 0;
            while enumerator.Next(500, &mut objects, &mut returned).is_ok() && returned > 0 {
                if let Some(obj) = objects[0].take() {
                    let mut name = VARIANT::default();
                    let mut temperature = VARIANT::default();
                    if obj.Get(w!("InstanceName"), 0, &mut name, None, None).is_ok()
                        && obj.Get(w!("Temperature"), 0, &mut temperature, None, None).is_ok()
                    {
                        if let Ok(value) = temperature.to_string().parse::<i32>() {
                            samples.push((name.to_string(), value));
                        }
                    }
                }
            }
        }

        Some(samples)
    }
}

fn select_package_temp(samples: &[(String, i32)]) -> Option<i32> {
    samples
        .iter()
        .find(|(name, temp)| name.ends_with(PACKAGE_INSTANCE_SUFFIX) && *temp > 0 && *temp < 125)
        .map(|(_, temp)| *temp)
}

#[cfg(test)]
mod tests {
    use super::select_package_temp;

    fn sample(name: &str, temp: i32) -> (String, i32) {
        (name.to_string(), temp)
    }

    #[test]
    fn selects_package_domain_only() {
        let samples = [sample(r"PCI\DEV_0", 61), sample(r"PCI\DEV_10", 29), sample(r"PCI\DEV_1", 60)];
        assert_eq!(select_package_temp(&samples), Some(61));
    }

    #[test]
    fn ignores_domain_ten_when_package_is_absent() {
        assert_eq!(select_package_temp(&[sample(r"PCI\DEV_10", 29)]), None);
    }

    #[test]
    fn rejects_implausible_values() {
        assert_eq!(select_package_temp(&[sample(r"PCI\DEV_0", 0)]), None);
        assert_eq!(select_package_temp(&[sample(r"PCI\DEV_0", 255)]), None);
    }
}
