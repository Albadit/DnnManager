//! IIS's "Web Service" performance counters (`IisManager.GetRequestsServed` / `GetSiteTraffic`): one instance per
//! site, named like it. Read with the PDH API by their English names, so it works in every Windows language - the C#
//! reads the category with `PerformanceCounterCategory.ReadCategory`. Both reads share one read of the counters made
//! less than a second ago, as the C# does.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use dnnm_application::project::SiteTraffic;

/// The raw values of one read: counter → site → value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebServiceSample {
    /// Every kind of request (GET, HEAD, POST…) - "Total Method Requests", not "Total Get Requests" alone.
    pub requests: HashMap<String, u64>,
    pub received: HashMap<String, u64>,
    pub sent: HashMap<String, u64>,
}

impl WebServiceSample {
    pub fn traffic(&self) -> HashMap<String, SiteTraffic> {
        self.received
            .iter()
            .filter_map(|(site, &received)| {
                let sent = *self.sent.get(site)?;
                Some((site.clone(), SiteTraffic { bytes_received: received, bytes_sent: sent }))
            })
            .collect()
    }
}

static LAST: Mutex<Option<(Instant, WebServiceSample)>> = Mutex::new(None);

/// The counters, read now - or as read less than a second ago; empty when they aren't there or can't be read.
pub fn web_service() -> WebServiceSample {
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, sample)) = last.as_ref()
        && at.elapsed() < Duration::from_secs(1)
    {
        return sample.clone();
    }
    let sample = read().unwrap_or_default();
    *last = Some((Instant::now(), sample.clone()));
    sample
}

#[cfg(windows)]
fn read() -> Option<WebServiceSample> {
    use windows::Win32::System::Performance::*;
    use windows::core::HSTRING;

    const PDH_MORE_DATA: u32 = 0x8000_07D2;
    const PATHS: [&str; 3] = [
        r"\Web Service(*)\Total Method Requests",
        r"\Web Service(*)\Total Bytes Received",
        r"\Web Service(*)\Total Bytes Sent",
    ];
    // SAFETY: a PDH query opened and closed here; each array is read into a buffer of the size PDH asked for, aligned
    // for its items, and the names it points to live in that buffer.
    unsafe {
        let mut query = PDH_HQUERY::default();
        if PdhOpenQueryW(None, 0, &mut query) != 0 {
            return None;
        }
        let mut counters = Vec::new();
        for path in PATHS {
            let mut counter = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(query, &HSTRING::from(path), 0, &mut counter) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            counters.push(counter);
        }
        if PdhCollectQueryData(query) != 0 {
            PdhCloseQuery(query);
            return None;
        }
        let mut values: Vec<HashMap<String, u64>> = Vec::new();
        for counter in counters {
            let (mut size, mut count) = (0u32, 0u32);
            let mut map = HashMap::new();
            if PdhGetRawCounterArrayW(counter, &mut size, &mut count, None) == PDH_MORE_DATA && size > 0 {
                let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
                let items = buffer.as_mut_ptr() as *mut PDH_RAW_COUNTER_ITEM_W;
                if PdhGetRawCounterArrayW(counter, &mut size, &mut count, Some(items)) == 0 {
                    for item in std::slice::from_raw_parts(items, count as usize) {
                        let name = item.szName.to_string().unwrap_or_default();
                        map.insert(name, item.RawValue.FirstValue.max(0) as u64);
                    }
                }
            }
            values.push(map);
        }
        PdhCloseQuery(query);
        let mut values = values.into_iter();
        Some(WebServiceSample {
            requests: values.next().unwrap_or_default(),
            received: values.next().unwrap_or_default(),
            sent: values.next().unwrap_or_default(),
        })
    }
}

#[cfg(not(windows))]
fn read() -> Option<WebServiceSample> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traffic_needs_both_counters() {
        let sample = WebServiceSample {
            requests: HashMap::new(),
            received: HashMap::from([("shop".to_string(), 10), ("blog".to_string(), 5)]),
            sent: HashMap::from([("shop".to_string(), 20)]),
        };
        let traffic = sample.traffic();
        assert_eq!(traffic["shop"], SiteTraffic { bytes_received: 10, bytes_sent: 20 });
        assert!(!traffic.contains_key("blog"));
    }

    #[test]
    fn reading_the_counters_never_fails() {
        // Read-only. Without IIS the category isn't there: empty, not an error.
        let sample = web_service();
        if let Some(total) = sample.requests.get("_Total") {
            assert!(*total < u64::MAX);
        }
        assert_eq!(web_service(), sample, "a second read within a second is the same read");
    }
}
