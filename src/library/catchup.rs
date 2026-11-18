//! Where a programme that already aired plays from: the provider's archive.

use crate::iptv::model::{CatchupKind, Channel, Programme};
use crate::iptv::{time, xtream};

/// The URL that plays `programme` from the archive, if the channel has one that
/// reaches back that far.
pub fn catchup_url(channel: &Channel, programme: &Programme, login: Option<&xtream::Login>, server_offset: i64) -> Option<String> {
    let catchup = channel.catchup.as_ref()?;
    let now = time::now();
    if programme.start > now || programme.start < now - i64::from(catchup.days) * 86_400 {
        return None;
    }
    let duration = (programme.stop.min(now) - programme.start).max(60);
    Some(match &catchup.kind {
        CatchupKind::Xtream { stream_id } => login?.timeshift_url(*stream_id, programme.start, (duration / 60) as u32 + 1, server_offset),
        CatchupKind::Template { source, append } => {
            let filled = fill_template(source, programme.start, programme.stop, now);
            if *append { format!("{}{filled}", channel.url) } else { filled }
        }
        CatchupKind::Shift => {
            let sep = if channel.url.contains('?') { '&' } else { '?' };
            format!("{}{sep}utc={}&lutc={now}", channel.url, programme.start)
        }
        CatchupKind::Flussonic => {
            let (base, _) = channel.url.rsplit_once('/')?;
            format!("{base}/index-{}-{duration}.m3u8", programme.start)
        }
    })
}

/// Fills a `catchup-source` template (the placeholders Kodi's IPTV Simple Client
/// understands).
fn fill_template(t: &str, start: i64, stop: i64, now: i64) -> String {
    let c = time::civil(start);
    let duration = stop - start;
    let mut s = t.to_string();
    for (k, v) in [
        ("{utc}", start.to_string()),
        ("${start}", start.to_string()),
        ("{start}", start.to_string()),
        ("${timestamp}", now.to_string()),
        ("{utcend}", stop.to_string()),
        ("${end}", stop.to_string()),
        ("{end}", stop.to_string()),
        ("{lutc}", now.to_string()),
        ("${now}", now.to_string()),
        ("{now}", now.to_string()),
        ("${offset}", (now - start).to_string()),
        ("${duration}", duration.to_string()),
        ("{duration}", duration.to_string()),
        ("{Y}", format!("{:04}", c.year)),
        ("{m}", format!("{:02}", c.month)),
        ("{d}", format!("{:02}", c.day)),
        ("{H}", format!("{:02}", c.hour)),
        ("{M}", format!("{:02}", c.minute)),
        ("{S}", format!("{:02}", c.second)),
    ] {
        s = s.replace(k, &v);
    }
    // `{duration:60}` (in minutes), `{offset:1}` (seconds back from now, divided).
    for (name, value) in [("duration", duration), ("offset", now - start)] {
        while let Some(i) = s.find(&format!("{{{name}:")) {
            let Some(end) = s[i..].find('}') else { break };
            let divisor: i64 = s[i + name.len() + 2..i + end].parse().unwrap_or(1).max(1);
            s.replace_range(i..i + end + 1, &(value / divisor).to_string());
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iptv::model::Catchup;

    fn channel(kind: CatchupKind, url: &str) -> Channel {
        Channel {
            key: "1".into(),
            name: "One".into(),
            number: None,
            logo: None,
            group: String::new(),
            url: url.into(),
            epg_id: None,
            catchup: Some(Catchup { days: 7, kind }),
            user_agent: None,
            referrer: None,
        }
    }

    #[test]
    fn fills_catchup_templates() {
        let start = 1_793_304_000; // 2026-10-29 20:00 UTC
        let t = fill_template(
            "http://cdn/{Y}/{m}/{d}/{H}-{M}?s={utc}&e={utcend}&d={duration:60}&o={offset:60}",
            start,
            start + 7200,
            start + 3600,
        );
        assert_eq!(t, "http://cdn/2026/10/29/20-00?s=1793304000&e=1793311200&d=120&o=60");
    }

    #[test]
    fn builds_catchup_urls() {
        let now = time::now();
        let p = Programme { start: now - 3600, stop: now - 600, title: String::new(), desc: String::new() };
        let shift = catchup_url(&channel(CatchupKind::Shift, "http://x/live.ts?token=1"), &p, None, 0).unwrap();
        assert!(shift.starts_with(&format!("http://x/live.ts?token=1&utc={}&lutc=", now - 3600)), "{shift}");
        let fl = catchup_url(&channel(CatchupKind::Flussonic, "http://x/ch1/index.m3u8"), &p, None, 0).unwrap();
        assert_eq!(fl, format!("http://x/ch1/index-{}-3000.m3u8", now - 3600));
        let old = Programme { start: now - 30 * 86_400, stop: now - 30 * 86_400 + 60, title: String::new(), desc: String::new() };
        assert!(catchup_url(&channel(CatchupKind::Shift, "http://x"), &old, None, 0).is_none(), "beyond the archive");
        let future = Programme { start: now + 60, stop: now + 120, title: String::new(), desc: String::new() };
        assert!(catchup_url(&channel(CatchupKind::Shift, "http://x"), &future, None, 0).is_none());
    }
}
