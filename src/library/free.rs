//! Free channels to start with: the lists the Free-TV project keeps, by country and
//! by topic. It takes only channels their broadcasters offer free to everyone (over
//! the air or on their own sites), nothing paid. Only the lists are named here;
//! what's in them comes fresh from the project each time the source loads.

/// Where the lists are.
const BASE: &str = "https://raw.githubusercontent.com/Free-TV/IPTV/master/playlists";

/// A list of free channels to add as a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Free {
    /// A country's channels; `code` is its ISO code (`uk` for the United Kingdom).
    Country { code: &'static str, list: &'static str, name: &'static str },
    /// Channels on one topic, from several countries.
    Topic { list: &'static str, name: &'static str },
}

impl Free {
    /// Where the list is.
    pub fn url(self) -> String {
        match self {
            Free::Country { list, .. } | Free::Topic { list, .. } => format!("{BASE}/playlist_{list}.m3u8"),
        }
    }

    /// The country or the topic, as shown.
    pub const fn name(self) -> &'static str {
        match self {
            Free::Country { name, .. } | Free::Topic { name, .. } => name,
        }
    }

    /// Every country with a list, by name.
    pub fn countries() -> impl Iterator<Item = Free> {
        COUNTRIES.iter().map(|&(code, list, name)| Free::Country { code, list, name })
    }

    /// Every topic with a list.
    pub fn topics() -> impl Iterator<Item = Free> {
        TOPICS.iter().map(|&(list, name)| Free::Topic { list, name })
    }

    /// The country the computer is set to, when it has a list: its region setting,
    /// else its time zone's country, else its language's region.
    pub fn home() -> Option<Free> {
        let locale = locale();
        let code = locale.as_deref().and_then(region_override).or_else(zone_country).or_else(|| region(locale.as_deref()?))?;
        Free::countries().find(|f| matches!(f, Free::Country { code: c, .. } if *c == code))
    }
}

/// The locale the computer is set to: `hr_HR.UTF-8`, `en-GB`, `en_US@rg=hrzzzz`.
fn locale() -> Option<String> {
    let env = ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().filter_map(|k| std::env::var(k).ok()).find(|v| region(v).is_some());
    if env.is_some() {
        return env;
    }
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("defaults").args(["read", "-g", "AppleLocale"]).output().ok()?;
        return Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    None
}

/// The region set apart from the language (`en_US@rg=hrzzzz` → `hr`).
fn region_override(locale: &str) -> Option<String> {
    let rg = locale.split_once("rg=")?.1.get(..2)?;
    rg.chars().all(|c| c.is_ascii_alphabetic()).then(|| code(rg))
}

/// The region of a locale (`hr_HR.UTF-8` → `hr`, `en_GB` → `uk`).
fn region(locale: &str) -> Option<String> {
    let base = locale.split(['.', '@']).next()?;
    let region = base.split(['_', '-']).skip(1).find(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic()))?;
    Some(code(region))
}

/// The list's code for a country's ISO code.
fn code(iso: &str) -> String {
    let code = iso.to_ascii_lowercase();
    if code == "gb" { "uk".into() } else { code }
}

/// The country of the computer's time zone (`Europe/Zagreb` → `hr`), from the
/// system's zone table.
fn zone_country() -> Option<String> {
    let zone = std::env::var("TZ").ok().map(|z| z.trim_start_matches(':').to_string()).or_else(|| {
        let link = std::fs::read_link("/etc/localtime").ok()?;
        let link = link.to_string_lossy();
        Some(link.split_once("zoneinfo/")?.1.to_string())
    })?;
    let table = std::fs::read_to_string("/usr/share/zoneinfo/zone.tab").ok()?;
    country_of(&table, &zone)
}

/// The country of `zone` in a zone table (`zone.tab`: code, place, zone, comment).
fn country_of(table: &str, zone: &str) -> Option<String> {
    table.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
        let mut cols = l.split('\t');
        let iso = cols.next()?;
        (cols.nth(1)? == zone).then(|| code(iso))
    })
}

/// Channels on one topic, from several countries.
const TOPICS: &[(&str, &str)] = &[
    ("zz_news_en", "News in English"),
    ("zz_news_es", "News in Spanish"),
    ("zz_news_ar", "News in Arabic"),
    ("zz_documentaries_en", "Documentaries in English"),
    ("zz_documentaries_ar", "Documentaries in Arabic"),
    ("zz_music_en", "Music in English"),
];

/// Countries with a list: ISO code (`uk` for the United Kingdom), the list's
/// name, the country's name.
const COUNTRIES: &[(&str, &str, &str)] = &[
    ("al", "albania", "Albania"),
    ("ad", "andorra", "Andorra"),
    ("ar", "argentina", "Argentina"),
    ("am", "armenia", "Armenia"),
    ("au", "australia", "Australia"),
    ("at", "austria", "Austria"),
    ("az", "azerbaijan", "Azerbaijan"),
    ("by", "belarus", "Belarus"),
    ("be", "belgium", "Belgium"),
    ("ba", "bosnia_and_herzegovina", "Bosnia and Herzegovina"),
    ("br", "brazil", "Brazil"),
    ("bg", "bulgaria", "Bulgaria"),
    ("ca", "canada", "Canada"),
    ("td", "chad", "Chad"),
    ("cl", "chile", "Chile"),
    ("cn", "china", "China"),
    ("cr", "costa_rica", "Costa Rica"),
    ("hr", "croatia", "Croatia"),
    ("cy", "cyprus", "Cyprus"),
    ("cz", "czech_republic", "Czech Republic"),
    ("dk", "denmark", "Denmark"),
    ("do", "dominican_republic", "Dominican Republic"),
    ("eg", "egypt", "Egypt"),
    ("ee", "estonia", "Estonia"),
    ("fo", "faroe_islands", "Faroe Islands"),
    ("fi", "finland", "Finland"),
    ("fr", "france", "France"),
    ("ge", "georgia", "Georgia"),
    ("de", "germany", "Germany"),
    ("gr", "greece", "Greece"),
    ("gl", "greenland", "Greenland"),
    ("hk", "hong_kong", "Hong Kong"),
    ("hu", "hungary", "Hungary"),
    ("is", "iceland", "Iceland"),
    ("in", "india", "India"),
    ("id", "indonesia", "Indonesia"),
    ("ir", "iran", "Iran"),
    ("iq", "iraq", "Iraq"),
    ("ie", "ireland", "Ireland"),
    ("il", "israel", "Israel"),
    ("it", "italy", "Italy"),
    ("jp", "japan", "Japan"),
    ("kz", "kazakhstan", "Kazakhstan"),
    ("ke", "kenya", "Kenya"),
    ("xk", "kosovo", "Kosovo"),
    ("lv", "latvia", "Latvia"),
    ("lb", "lebanon", "Lebanon"),
    ("lt", "lithuania", "Lithuania"),
    ("lu", "luxembourg", "Luxembourg"),
    ("mo", "macau", "Macao"),
    ("mt", "malta", "Malta"),
    ("mx", "mexico", "Mexico"),
    ("md", "moldova", "Moldova"),
    ("mc", "monaco", "Monaco"),
    ("mn", "mongolia", "Mongolia"),
    ("me", "montenegro", "Montenegro"),
    ("nl", "netherlands", "Netherlands"),
    ("ng", "nigeria", "Nigeria"),
    ("kp", "north_korea", "North Korea"),
    ("mk", "north_macedonia", "North Macedonia"),
    ("no", "norway", "Norway"),
    ("py", "paraguay", "Paraguay"),
    ("pe", "peru", "Peru"),
    ("pl", "poland", "Poland"),
    ("pt", "portugal", "Portugal"),
    ("qa", "qatar", "Qatar"),
    ("ro", "romania", "Romania"),
    ("ru", "russia", "Russia"),
    ("sm", "san_marino", "San Marino"),
    ("sa", "saudi_arabia", "Saudi Arabia"),
    ("rs", "serbia", "Serbia"),
    ("sk", "slovakia", "Slovakia"),
    ("si", "slovenia", "Slovenia"),
    ("so", "somalia", "Somalia"),
    ("kr", "korea", "South Korea"),
    ("es", "spain", "Spain"),
    ("se", "sweden", "Sweden"),
    ("ch", "switzerland", "Switzerland"),
    ("tw", "taiwan", "Taiwan"),
    ("tt", "trinidad", "Trinidad and Tobago"),
    ("tm", "turkmenistan", "Turkmenistan"),
    ("tr", "turkey", "Turkey"),
    ("ua", "ukraine", "Ukraine"),
    ("ae", "united_arab_emirates", "United Arab Emirates"),
    ("uk", "uk", "United Kingdom"),
    ("us", "usa", "United States"),
    ("ve", "venezuela", "Venezuela"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_region_of_a_locale() {
        assert_eq!(region("hr_HR.UTF-8").as_deref(), Some("hr"));
        assert_eq!(region("en-GB").as_deref(), Some("uk"));
        assert_eq!(region("sr_Latn_RS").as_deref(), Some("rs"));
        assert_eq!(region("C"), None);
        assert_eq!(region("en"), None);
        assert_eq!(region_override("en_US@rg=hrzzzz").as_deref(), Some("hr"));
        assert_eq!(region_override("en_US"), None);
    }

    #[test]
    fn finds_the_country_of_a_time_zone() {
        let table = "# comment\nHR\t+4548+01558\tEurope/Zagreb\nGB\t+513030-0000731\tEurope/London\n";
        assert_eq!(country_of(table, "Europe/Zagreb").as_deref(), Some("hr"));
        assert_eq!(country_of(table, "Europe/London").as_deref(), Some("uk"));
        assert_eq!(country_of(table, "Asia/Tokyo"), None);
    }

    #[test]
    fn links_the_lists() {
        let hr = Free::countries().find(|f| f.name() == "Croatia").unwrap();
        assert_eq!(hr.url(), "https://raw.githubusercontent.com/Free-TV/IPTV/master/playlists/playlist_croatia.m3u8");
        let news = Free::topics().next().unwrap();
        assert_eq!(news.url(), "https://raw.githubusercontent.com/Free-TV/IPTV/master/playlists/playlist_zz_news_en.m3u8");
    }
}
