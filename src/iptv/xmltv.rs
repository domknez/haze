//! XMLTV guides: `<programme start="…" stop="…" channel="id"><title>…</title>
//! <desc>…</desc></programme>`. Provider guides run to hundreds of megabytes, so the
//! file is read as a stream and only programmes of known channels, near now, are kept.

use super::model::{Guide, Programme};
use super::time::parse_xmltv;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use std::collections::HashSet;
use std::io::BufRead;

/// Programmes this far back are kept (for the archive) …
pub const KEEP_BEFORE: i64 = 7 * 86_400;
/// … and this far ahead.
pub const KEEP_AFTER: i64 = 3 * 86_400;

#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum Field {
    #[default]
    None,
    Title,
    Desc,
}

/// Read a guide, keeping programmes of `channels` (all when none) between
/// `now - KEEP_BEFORE` and `now + KEEP_AFTER`.
pub fn parse(input: impl BufRead, channels: Option<&HashSet<String>>, now: i64) -> Result<Guide, quick_xml::Error> {
    // Text is not trimmed while reading: `Studio &#8211; preview` comes in three
    // pieces, and the spaces between them count.
    let mut reader = quick_xml::Reader::from_reader(input);
    let mut guide = Guide::default();
    let mut buf = Vec::new();
    // The programme being read: its channel and what's known of it so far.
    let mut current: Option<(String, Programme)> = None;
    let mut field = Field::None;
    let (from, until) = (now - KEEP_BEFORE, now + KEEP_AFTER);
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => match e.name().as_ref() {
                "programme" => current = programme(&e).filter(|(ch, p)| p.stop > from && p.start < until && channels.is_none_or(|c| c.contains(ch))),
                "title" if current.as_ref().is_some_and(|(_, p)| p.title.is_empty()) => field = Field::Title,
                "desc" if current.as_ref().is_some_and(|(_, p)| p.desc.is_empty()) => field = Field::Desc,
                _ => field = Field::None,
            },
            Event::Text(t) => push(&mut current, field, &t.xml10_content()),
            Event::CData(t) => push(&mut current, field, &t.xml10_content()),
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref() {
                    Ok(Some(c)) => Some(c.to_string()),
                    _ => quick_xml::escape::resolve_predefined_entity(&r).map(str::to_string),
                };
                if let Some(s) = resolved {
                    push(&mut current, field, &s);
                }
            }
            Event::End(e) => match e.name().as_ref() {
                "programme" => {
                    if let Some((channel, mut p)) = current.take() {
                        p.title = p.title.trim().to_string();
                        p.desc = p.desc.trim().to_string();
                        guide.programmes.entry(channel).or_default().push(p);
                    }
                    field = Field::None;
                }
                _ => field = Field::None,
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    for list in guide.programmes.values_mut() {
        list.sort_by_key(|p| p.start);
        // Guides merged from several sources repeat programmes.
        list.dedup_by_key(|p| p.start);
        list.shrink_to_fit();
    }
    Ok(guide)
}

fn programme(e: &BytesStart) -> Option<(String, Programme)> {
    let attr = |name: &str| {
        e.try_get_attribute(name).ok().flatten().and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
    };
    let start = parse_xmltv(&attr("start")?)?;
    // Some guides leave out `stop`; the next programme's start fills it in later.
    let stop = attr("stop").and_then(|s| parse_xmltv(&s)).unwrap_or(start + 3600);
    Some((attr("channel")?, Programme { start, stop, title: String::new(), desc: String::new() }))
}

fn push(current: &mut Option<(String, Programme)>, field: Field, text: &str) {
    let Some((_, p)) = current else { return };
    match field {
        Field::Title => p.title.push_str(text),
        Field::Desc => p.desc.push_str(text),
        Field::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_793_304_000; // 2026-10-29 20:00 UTC

    const GUIDE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv generator-info-name="test">
  <channel id="arena1.hr"><display-name>Arena Sport 1</display-name></channel>
  <programme start="20261029210000 +0100" stop="20261029230000 +0100" channel="arena1.hr">
    <title lang="hr">Dinamo &amp; Hajduk</title>
    <title lang="en">Dinamo v Hajduk</title>
    <desc lang="hr"><![CDATA[Derby <live>]]></desc>
    <category>Sport</category>
  </programme>
  <programme start="20261029190000 +0000" stop="20261029200000 +0000" channel="arena1.hr">
    <title>Studio &#8211; preview</title>
  </programme>
  <programme start="20261029200000 +0000" stop="20261029220000 +0000" channel="arena1.hr"><title>Duplicate</title></programme>
  <programme start="20261029200000 +0000" stop="20261029210000 +0000" channel="unknown"><title>Other</title></programme>
  <programme start="20250101000000 +0000" stop="20250101010000 +0000" channel="arena1.hr"><title>Long ago</title></programme>
</tv>"#;

    #[test]
    fn reads_programmes_of_known_channels() {
        let wanted: HashSet<String> = ["arena1.hr".to_string()].into();
        let g = parse(GUIDE.as_bytes(), Some(&wanted), NOW).unwrap();
        assert_eq!(g.programmes.len(), 1);
        let list = &g.programmes["arena1.hr"];
        assert_eq!(list.len(), 2, "sorted, deduplicated, old ones dropped");
        assert_eq!(list[0].title, "Studio – preview");
        assert_eq!((list[1].start, list[1].stop), (NOW, NOW + 7200));
        assert_eq!(list[1].title, "Dinamo & Hajduk", "first title only");
        assert_eq!(list[1].desc, "Derby <live>");
    }

    #[test]
    fn keeps_everything_without_a_channel_list() {
        let g = parse(GUIDE.as_bytes(), None, NOW).unwrap();
        assert_eq!(g.programmes.len(), 2);
    }
}
