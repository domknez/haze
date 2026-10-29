//! What the rail browses: the tab, the group picked in each, the search, the open
//! series and its episodes, and the list worked out from them.

use super::load::Load;
use super::session::Playing;
use crate::iptv::model::{Catalog, Episode};
use crate::library::Marks;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Live,
    Favorites,
    Recent,
    Movies,
    Series,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Live, Tab::Favorites, Tab::Recent, Tab::Movies, Tab::Series];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Live => "Live",
            Tab::Favorites => "Favorites",
            Tab::Recent => "Recent",
            Tab::Movies => "Movies",
            Tab::Series => "Series",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Tab::Live => "live",
            Tab::Favorites => "favorites",
            Tab::Recent => "recent",
            Tab::Movies => "movies",
            Tab::Series => "series",
        }
    }

    pub fn from_key(k: &str) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| t.key() == k)
    }
}

/// The groups of the channels, the movies or the series, each with how many it
/// holds (counted once per catalog), and the one picked (None for all).
#[derive(Default)]
pub struct Groups {
    pub picked: Option<String>,
    pub counts: Vec<(String, usize)>,
}

pub struct Browse {
    pub tab: Tab,
    pub channels: Groups,
    pub movies: Groups,
    pub series: Groups,
    pub search: String,
    /// Narrows the column of groups.
    pub group_filter: String,
    /// The series whose episodes the rail lists (its index in the catalog), and them.
    pub open_series: Option<usize>,
    pub episodes: Load<Vec<Episode>>,
    /// Groups the viewer hid, left out everywhere unless `show_hidden`.
    pub hidden: BTreeSet<String>,
    pub show_hidden: bool,
    /// Indices of what the rail lists, and what they were worked out for.
    pub list: Vec<usize>,
    list_key: Option<ListKey>,
}

/// What the rail's list was worked out for.
#[derive(Clone, Debug, PartialEq)]
struct ListKey {
    tab: Tab,
    group: Option<String>,
    search: String,
    /// How big the catalog is (a new one lists again).
    size: usize,
    marks: u64,
    /// The open series and how many episodes it has, once they're in.
    episodes: Option<(usize, usize)>,
}

impl Browse {
    pub fn new(tab: Tab) -> Browse {
        Browse {
            tab,
            channels: Groups::default(),
            movies: Groups::default(),
            series: Groups::default(),
            search: String::new(),
            group_filter: String::new(),
            open_series: None,
            episodes: Load::Idle,
            hidden: BTreeSet::new(),
            show_hidden: false,
            list: Vec::new(),
            list_key: None,
        }
    }

    /// Forget the groups and the series of the source left.
    pub fn reset(&mut self) {
        self.channels = Groups::default();
        self.movies = Groups::default();
        self.series = Groups::default();
        self.close_series();
    }

    /// A catalog came in: count its groups, and pick `last_group` among the channels
    /// if it still has it.
    pub fn catalog_arrived(&mut self, c: &Catalog, last_group: Option<&String>) {
        self.count_groups(c);
        self.channels.picked = last_group.filter(|g| self.shows(g) && c.groups.contains(g)).cloned();
    }

    /// Count the groups shown, after hiding or showing some.
    pub fn count_groups(&mut self, c: &Catalog) {
        let shown = |groups: &[String]| groups.iter().filter(|g| self.shows(g)).cloned().collect::<Vec<_>>();
        let (channels, movies, series) = (shown(&c.groups), shown(&c.movie_groups), shown(&c.series_groups));
        self.channels.counts = count(&channels, c.channels.iter().map(|ch| &ch.group));
        self.movies.counts = count(&movies, c.movies.iter().map(|m| &m.group));
        self.series.counts = count(&series, c.series.iter().map(|s| &s.group));
        for groups in [&mut self.channels, &mut self.movies, &mut self.series] {
            if groups.picked.as_ref().is_some_and(|g| !groups.counts.iter().any(|(n, _)| n == g)) {
                groups.picked = None;
            }
        }
        self.relist();
    }

    /// Whether `group`'s entries show: it isn't hidden, or hidden ones show.
    pub fn shows(&self, group: &str) -> bool {
        self.show_hidden || !self.hidden.contains(group)
    }

    /// Work the list out again on the next refresh.
    pub fn relist(&mut self) {
        self.list_key = None;
    }

    /// The groups of the tab on screen.
    pub fn groups(&self) -> &Groups {
        match self.tab {
            Tab::Movies => &self.movies,
            Tab::Series => &self.series,
            _ => &self.channels,
        }
    }

    pub fn pick_group(&mut self, group: Option<String>) {
        let groups = match self.tab {
            Tab::Movies => &mut self.movies,
            Tab::Series => {
                self.close_series();
                &mut self.series
            }
            _ => &mut self.channels,
        };
        groups.picked = group;
    }

    /// Back from the episodes to the series.
    pub fn close_series(&mut self) {
        self.open_series = None;
        self.episodes = Load::Idle;
        self.list_key = None;
    }

    /// What the rail lists, worked out again only when something it depends on changed.
    pub fn refresh(&mut self, catalog: Option<&Catalog>, marks: &Marks, source: &str, marks_serial: u64) {
        let Some(catalog) = catalog else {
            self.list.clear();
            return;
        };
        let group = self.groups().picked.clone();
        let episodes = self.open_series.and_then(|i| Some((i, self.episodes.ready()?.len())));
        let key = ListKey {
            tab: self.tab,
            group: group.clone(),
            search: self.search.clone(),
            size: catalog.channels.len() + catalog.movies.len() + catalog.series.len(),
            marks: marks_serial,
            episodes,
        };
        if self.list_key.as_ref() == Some(&key) {
            return;
        }
        let needle = self.search.trim().to_lowercase();
        let matches = |name: &str| needle.is_empty() || name.to_lowercase().contains(&needle);
        let in_group = |g: &String| group.as_ref().map_or_else(|| self.shows(g), |picked| g == picked);
        let by_key = |k: &str| catalog.channels.iter().position(|c| c.key == k);
        let named = |i: &usize| matches(&catalog.channels[*i].name) && self.shows(&catalog.channels[*i].group);
        self.list = match self.tab {
            Tab::Live => indices(&catalog.channels, |c| in_group(&c.group) && matches(&c.name)),
            Tab::Favorites => marks.favorites(source).filter_map(by_key).filter(named).collect(),
            Tab::Recent => marks.recent(source).filter_map(by_key).filter(named).collect(),
            Tab::Movies => indices(&catalog.movies, |m| in_group(&m.group) && matches(&m.name)),
            Tab::Series if self.open_series.is_some() => (0..episodes.map_or(0, |(_, n)| n)).collect(),
            Tab::Series => indices(&catalog.series, |s| in_group(&s.group) && matches(&s.name)),
        };
        self.list_key = Some(key);
    }

    /// Episode `i` of the open series.
    pub fn episode(&self, catalog: &Catalog, i: usize) -> Option<Playing> {
        let series = catalog.series.get(self.open_series?)?.key.clone();
        Some(Playing::Episode { series, episode: self.episodes.ready()?.get(i)?.clone() })
    }

    /// The episode after `playing`, if its series is open.
    pub fn next_episode(&self, catalog: &Catalog, playing: &Playing) -> Option<Playing> {
        let Playing::Episode { series, episode } = playing else { return None };
        let open = catalog.series.get(self.open_series?)?;
        let list = self.episodes.ready().filter(|_| &open.key == series)?;
        let at = list.iter().position(|e| e.key == episode.key)?;
        Some(Playing::Episode { series: series.clone(), episode: list.get(at + 1)?.clone() })
    }

    /// What's `d` entries on from `current` in the list (channel zapping), or the
    /// first entry when `current` isn't in it. The list of series has no order to
    /// zap through; the episodes of an open one do.
    pub fn step(&self, catalog: &Catalog, current: Option<&str>, d: i32) -> Option<Playing> {
        if self.list.is_empty() {
            return None;
        }
        let episodes = self.episodes.ready().map_or(&[][..], Vec::as_slice);
        let key_at = |i: usize| match self.tab {
            Tab::Movies => Some(catalog.movies[i].key.as_str()),
            Tab::Series => self.open_series.and(episodes.get(i)).map(|e| e.key.as_str()),
            _ => Some(catalog.channels[i].key.as_str()),
        };
        let pos = current.and_then(|k| self.list.iter().position(|&i| key_at(i) == Some(k)));
        let next = self.list[pos.map_or(0, |p| (p as i32 + d).rem_euclid(self.list.len() as i32) as usize)];
        match self.tab {
            Tab::Movies => Some(Playing::Movie { key: catalog.movies[next].key.clone() }),
            Tab::Series => self.episode(catalog, next),
            _ => Some(Playing::Live { key: catalog.channels[next].key.clone() }),
        }
    }
}

/// Each of `groups` with how many of `of` are in it.
fn count<'a>(groups: &[String], of: impl Iterator<Item = &'a String>) -> Vec<(String, usize)> {
    let mut n: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for g in of {
        *n.entry(g.as_str()).or_default() += 1;
    }
    groups.iter().map(|g| (g.clone(), n.get(g.as_str()).copied().unwrap_or(0))).collect()
}

fn indices<T>(items: &[T], keep: impl Fn(&T) -> bool) -> Vec<usize> {
    items.iter().enumerate().filter(|(_, x)| keep(x)).map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iptv::m3u;

    const LIST: &str = r#"#EXTM3U
#EXTINF:-1 group-title="News",News One
http://line.tv:80/user/pass/1
#EXTINF:-1 group-title="News",News Two
http://line.tv:80/user/pass/2
#EXTINF:-1 group-title="Sport",Sport One
http://line.tv:80/user/pass/3
#EXTINF:-1 group-title="Films",Dune (2021)
http://line.tv:80/movie/user/pass/5.mkv
#EXTINF:-1 group-title="Drama",Dark S1 E1
http://line.tv:80/series/user/pass/101.mkv
#EXTINF:-1 group-title="Drama",Dark S1 E2
http://line.tv:80/series/user/pass/102.mkv
"#;

    fn browsing(tab: Tab) -> (Browse, Catalog) {
        let catalog = m3u::parse(LIST);
        let mut b = Browse::new(tab);
        b.catalog_arrived(&catalog, None);
        (b, catalog)
    }

    fn listed(b: &mut Browse, c: &Catalog) -> Vec<String> {
        b.refresh(Some(c), &Marks::default(), "src", 0);
        b.list.iter().map(|&i| c.channels[i].name.clone()).collect()
    }

    #[test]
    fn counts_groups_and_keeps_the_last_one_picked() {
        let catalog = m3u::parse(LIST);
        let mut b = Browse::new(Tab::Live);
        b.catalog_arrived(&catalog, Some(&"Sport".to_string()));
        assert_eq!(b.channels.counts, [("News".to_string(), 2), ("Sport".to_string(), 1)]);
        assert_eq!(b.channels.picked.as_deref(), Some("Sport"));
        b.catalog_arrived(&catalog, Some(&"Gone".to_string()));
        assert_eq!(b.channels.picked, None);
    }

    #[test]
    fn lists_the_group_and_search_on_screen() {
        let (mut b, c) = browsing(Tab::Live);
        assert_eq!(listed(&mut b, &c), ["News One", "News Two", "Sport One"]);
        b.pick_group(Some("News".into()));
        assert_eq!(listed(&mut b, &c), ["News One", "News Two"]);
        b.search = "two".into();
        assert_eq!(listed(&mut b, &c), ["News Two"]);
    }

    #[test]
    fn each_tab_keeps_its_own_group() {
        let (mut b, _) = browsing(Tab::Live);
        b.pick_group(Some("News".into()));
        b.tab = Tab::Movies;
        assert_eq!(b.groups().picked, None);
        b.pick_group(Some("Films".into()));
        b.tab = Tab::Live;
        assert_eq!(b.groups().picked.as_deref(), Some("News"));
    }

    #[test]
    fn hidden_groups_stay_out_until_shown() {
        let (mut b, c) = browsing(Tab::Live);
        b.pick_group(Some("Sport".into()));
        b.hidden.insert("Sport".into());
        b.count_groups(&c);
        assert_eq!(b.channels.counts, [("News".to_string(), 2)]);
        assert_eq!(b.channels.picked, None);
        assert_eq!(listed(&mut b, &c), ["News One", "News Two"]);
        b.search = "one".into();
        assert_eq!(listed(&mut b, &c), ["News One"]);
        b.show_hidden = true;
        b.count_groups(&c);
        assert_eq!(listed(&mut b, &c), ["News One", "Sport One"]);
    }

    #[test]
    fn zaps_around_the_list() {
        let (mut b, c) = browsing(Tab::Live);
        b.refresh(Some(&c), &Marks::default(), "src", 0);
        let live = |k: &str| Some(Playing::Live { key: c.channels.iter().find(|ch| ch.name == k).unwrap().key.clone() });
        let key = |k: &str| live(k).map(|p| p.key().to_string()).unwrap();
        assert_eq!(b.step(&c, Some(&key("News Two")), 1), live("Sport One"));
        assert_eq!(b.step(&c, Some(&key("Sport One")), 1), live("News One"));
        assert_eq!(b.step(&c, Some(&key("News One")), -1), live("Sport One"));
        assert_eq!(b.step(&c, None, 1), live("News One"));
    }

    #[test]
    fn plays_the_next_episode_of_the_open_series() {
        let (mut b, c) = browsing(Tab::Series);
        b.open_series = Some(0);
        b.episodes = Load::Ready(c.series[0].episodes.clone());
        b.refresh(Some(&c), &Marks::default(), "src", 0);
        let first = b.episode(&c, 0).unwrap();
        let second = b.next_episode(&c, &first).unwrap();
        assert_eq!(second, b.episode(&c, 1).unwrap());
        assert_eq!(b.next_episode(&c, &second), None);
        assert_eq!(b.step(&c, Some(first.key()), 1), Some(second));
        b.close_series();
        assert_eq!(b.step(&c, None, 1), None);
    }
}
