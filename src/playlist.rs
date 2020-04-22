use crate::album::Album;
use crate::album::AlbumItem;
use crate::album::Error;
use log::warn;
use selector::Selector;
use std::time::Duration;

/// Build a playlist given stream of available contents.
///
/// First decide the `min_size` to be present on the list.
/// Then builder tries to fill the list with "fresh items" where the
/// "refreshness" is determined if the timestamp is greater than
/// `fresh_retention` ago.
/// If "fresh items" couldn't fill up list until `min_size`, "old items"
/// which has timestamp less than `fresh_retention` ago are randomly
/// selected and filled in for the remaining slots.
/// If "fresh items" were found more than `min_size`, the playlist size
/// will be extended up to `max_size`.
pub struct PlaylistBuilder {
    /// Expected minimum items to be present in the list
    min_size: usize,
    /// Maximum items to be present in the list
    max_size: usize,
    /// Time threshold to decide if an item is "fresh" or not
    fresh_retention: Duration,
}

impl PlaylistBuilder {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn min_size(mut self, min_size: usize) -> Self {
        self.min_size = min_size;
        self
    }

    pub fn max_size(mut self, max_size: usize) -> Self {
        self.max_size = max_size;
        self
    }

    pub fn fresh_retention(mut self, fresh_retention: Duration) -> Self {
        self.fresh_retention = fresh_retention;
        self
    }

    pub fn updated<'a, T: Album>(
        &self,
        album: &T,
        playlist: &'a [T::Item],
    ) -> Result<Option<Vec<T::Item>>, T::E> {
        let updated = self.do_build(
            Selectors::new(vec![Box::new(selector::PreviousItemSelector::new(
                self.fresh_retention,
                self.max_size,
                playlist.iter(),
            ))]),
            album,
        )?;
        Ok(if updated == playlist {
            None
        } else {
            Some(updated)
        })
    }

    pub fn build<T: Album>(&self, album: &T) -> Result<Vec<T::Item>, T::E> {
        self.do_build(
            Selectors::new(vec![
                Box::new(selector::FreshItemSelector::new(self.fresh_retention)),
                Box::new(selector::OldItemSelector::new(self.min_size)),
            ]),
            album,
        )
    }

    fn do_build<T: Album>(
        &self,
        mut selectors: Selectors<T::Item>,
        album: &T,
    ) -> Result<Vec<T::Item>, T::E> {
        for item in album.items() {
            if selectors.locked_count() == self.max_size {
                break;
            }

            let item = match item {
                Ok(item) => item,
                Err(e) => {
                    if e.is_fatal() {
                        return Err(e);
                    } else {
                        warn!("Skipping item by error: {}", e);
                        continue;
                    }
                }
            };

            selectors.consume(item);
        }
        Ok(selectors.select(self.min_size, self.max_size))
    }
}

impl Default for PlaylistBuilder {
    fn default() -> Self {
        PlaylistBuilder {
            min_size: 30,
            max_size: 100,
            fresh_retention: Duration::from_secs(3600 * 24 * 14), // 2 weeks
        }
    }
}

struct Selectors<'a, T: AlbumItem> {
    impls: Vec<Box<dyn Selector<T> + 'a>>,
}

impl<'a, T: AlbumItem> Selectors<'a, T> {
    fn new(impls: Vec<Box<dyn Selector<T> + 'a>>) -> Self {
        Self { impls }
    }

    fn consume(&mut self, mut item: T) {
        for selector in &mut self.impls {
            if let Some(it) = selector.take(item) {
                item = it;
            } else {
                break;
            }
        }
    }

    fn locked_count(&self) -> usize {
        self.impls.iter().map(|s| s.locked_count()).sum()
    }

    fn select(self, min_count: usize, max_count: usize) -> Vec<T> {
        let mut items = Vec::new();
        'outer: for selector in self.impls {
            let mut locked = selector.locked_count();
            for item in selector.drain() {
                if items.len() >= max_count {
                    break 'outer;
                }
                if items.len() >= min_count && locked == 0 {
                    break;
                }
                if locked > 0 {
                    locked -= 1;
                }
                items.push(item);
            }
        }
        items
    }
}

mod selector {
    use crate::album::AlbumItem;
    use log::debug;
    use rand::Rng;
    use std::cmp::Reverse;
    use std::collections::HashMap;
    use std::fmt::Debug;
    use std::time::Duration;
    use std::time::SystemTime;

    pub(super) trait Selector<I: AlbumItem> {
        fn take(&mut self, item: I) -> Option<I>;

        fn locked_count(&self) -> usize;

        fn drain(self: Box<Self>) -> Box<dyn Iterator<Item = I>>;
    }

    pub(super) struct FreshItemSelector<I> {
        min_fresh_time: SystemTime,
        items: Vec<I>,
    }

    impl<T> FreshItemSelector<T> {
        pub(super) fn new(fresh_retention: Duration) -> Self {
            Self {
                min_fresh_time: SystemTime::now() - fresh_retention,
                items: Vec::new(),
            }
        }
    }

    impl<I: AlbumItem + 'static> Selector<I> for FreshItemSelector<I> {
        fn take(&mut self, item: I) -> Option<I> {
            if item.created_time() >= self.min_fresh_time {
                debug!(
                    "Adding item as FRESH; id={}, time={}",
                    item.id(),
                    item.created_time()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                );
                self.items.push(item);
                return None;
            }
            Some(item)
        }

        fn locked_count(&self) -> usize {
            self.items.len()
        }

        fn drain(mut self: Box<Self>) -> Box<dyn Iterator<Item = I>> {
            self.items
                .sort_unstable_by_key(|item| Reverse(item.created_time()));
            Box::new(self.items.into_iter())
        }
    }

    pub(super) struct OldItemSelector<I: Debug> {
        max_items: usize,
        rand_slots: RandomSlots<I>,
    }

    impl<I: AlbumItem> OldItemSelector<I> {
        pub(super) fn new(max_items: usize) -> Self {
            Self {
                max_items,
                rand_slots: RandomSlots::new(max_items),
            }
        }
    }

    impl<I: AlbumItem + 'static> Selector<I> for OldItemSelector<I> {
        fn take(&mut self, item: I) -> Option<I> {
            debug!(
                "Adding item as OLD; id={}, time={}",
                item.id(),
                item.created_time()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            );
            self.rand_slots.push(item)
        }

        fn locked_count(&self) -> usize {
            0
        }

        fn drain(mut self: Box<Self>) -> Box<dyn Iterator<Item = I>> {
            Box::new((0..self.max_items).flat_map(move |_| self.rand_slots.pick_random()))
        }
    }

    pub(super) struct PreviousItemSelector<'a, I> {
        max_items: usize,
        prev_items: Vec<Option<I>>,
        order_map: HashMap<&'a str, usize>,
        newest_time: SystemTime,
        fresh_selector: FreshItemSelector<I>,
    }

    impl<'a, I: AlbumItem + 'a> PreviousItemSelector<'a, I> {
        pub(super) fn new<C: Iterator<Item = &'a I>>(
            fresh_retention: Duration,
            max_items: usize,
            prev_items_iter: C,
        ) -> Self {
            let mut newest_time = SystemTime::UNIX_EPOCH;
            let mut order_map = HashMap::new();
            for (i, item) in prev_items_iter.enumerate() {
                if item.created_time() > newest_time {
                    newest_time = item.created_time();
                }
                order_map.insert(item.id(), i);
            }
            let mut prev_items = Vec::with_capacity(order_map.len());
            for _ in 0..order_map.len() {
                prev_items.push(None);
            }

            Self {
                max_items,
                prev_items,
                order_map,
                newest_time,
                fresh_selector: FreshItemSelector::new(fresh_retention),
            }
        }

        fn is_newest(&self, item: &I) -> bool {
            item.created_time() > self.newest_time
        }
    }

    impl<'a, I: AlbumItem + 'static> Selector<I> for PreviousItemSelector<'a, I> {
        fn take(&mut self, item: I) -> Option<I> {
            if let Some(&slot) = self.order_map.get(item.id()) {
                debug!(
                    "Re-selecting item id={}, time={}",
                    item.id(),
                    item.created_time()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                );
                self.prev_items[slot].replace(item);
                return None;
            }

            if self.is_newest(&item)
                && self.prev_items.len() + self.fresh_selector.locked_count() < self.max_items
            {
                debug!(
                    "Adding new item id={}, time={}",
                    item.id(),
                    item.created_time()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                );
                return self.fresh_selector.take(item);
            }
            Some(item)
        }

        fn locked_count(&self) -> usize {
            self.prev_items.iter().filter(|b| b.is_some()).count()
                + self.fresh_selector.locked_count()
        }

        fn drain(self: Box<Self>) -> Box<dyn Iterator<Item = I>> {
            let prev_items: Vec<_> = self.prev_items.into_iter().filter_map(|v| v).collect();
            Box::new(
                Box::new(self.fresh_selector)
                    .drain()
                    .chain(prev_items.into_iter()),
            )
        }
    }

    struct RandomSlots<T: std::fmt::Debug> {
        capacity: usize,
        slots: Vec<Option<T>>,
        rng: rand::rngs::ThreadRng,
        count: usize,
    }

    impl<T: std::fmt::Debug> RandomSlots<T> {
        fn new(capacity: usize) -> Self {
            RandomSlots {
                capacity,
                slots: Vec::with_capacity(capacity),
                rng: rand::thread_rng(),
                count: 0,
            }
        }

        fn push(&mut self, item: T) -> Option<T> {
            self.count += 1;

            if self.slots.len() < self.capacity {
                self.slots.push(Some(item));
            } else {
                // Special thanks: Tom Tsuruhara
                let p = self.rng.gen_range(0, self.count);
                if p >= self.capacity {
                    return Some(item);
                }
                self.slots[p].replace(item);
            }
            None
        }

        fn pick_random(&mut self) -> Option<T> {
            if self.slots.is_empty() {
                return None;
            }

            let start = self.rng.gen_range(0, self.slots.len());
            let mut i = start;
            while self.slots[i].is_none() {
                i = (i + 1) % self.slots.len();
                if i == start {
                    return None;
                }
            }
            self.slots[i].take()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::album::{self, Album, AlbumItem, MediaType};
    use failure::{self, Fail};
    use std::path::Path;
    use std::time::SystemTime;

    struct MockAlbum {
        items: Vec<(&'static str, SystemTime)>,
    }

    impl Album for MockAlbum {
        type E = AlbumError;
        type Item = MockAlbumItem;
        type Items = MockItems;

        fn items(&self) -> Self::Items {
            MockItems(self.items.clone().into_iter())
        }

        fn prepare_item<P: AsRef<Path>>(
            &self,
            _item: &Self::Item,
            _path: P,
        ) -> Result<(), Self::E> {
            panic!("not implemented");
        }
    }

    struct MockItems(std::vec::IntoIter<(&'static str, SystemTime)>);

    impl Iterator for MockItems {
        type Item = Result<MockAlbumItem, AlbumError>;

        fn next(&mut self) -> Option<Self::Item> {
            self.0.next().map(|(id, t)| Ok(MockAlbumItem(id, t)))
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct MockAlbumItem(&'static str, SystemTime);

    impl AlbumItem for MockAlbumItem {
        fn id(&self) -> &str {
            &self.0
        }

        fn path(&self) -> &Path {
            panic!("not implemented")
        }

        fn media_type(&self) -> MediaType {
            panic!("not implemented");
        }

        fn created_time(&self) -> SystemTime {
            self.1
        }
    }

    #[derive(Debug, Fail)]
    #[fail(display = "error")]
    struct AlbumError {}

    impl album::Error for AlbumError {
        fn is_fatal(&self) -> bool {
            false
        }
    }

    struct Times {
        fresh_retention: Duration,
        base: SystemTime,
        news: u64,
        olds: u64,
    }

    impl Times {
        fn new() -> Self {
            Times {
                fresh_retention: Duration::from_secs(3600),
                base: SystemTime::now(),
                news: 0,
                olds: 0,
            }
        }

        fn fresh(&mut self, name: &'static str) -> (&'static str, SystemTime) {
            self.news += 1;
            (name, self.base - Duration::from_secs(self.news))
        }

        fn old(&mut self, name: &'static str) -> (&'static str, SystemTime) {
            self.olds += 1;
            (
                name,
                self.base - self.fresh_retention - Duration::from_secs(self.olds),
            )
        }
    }

    fn album(items: Vec<(&'static str, SystemTime)>) -> MockAlbum {
        MockAlbum { items }
    }

    fn names(playlist: Vec<MockAlbumItem>) -> Vec<&'static str> {
        playlist
            .into_iter()
            .map(|MockAlbumItem(id, _)| id)
            .collect()
    }

    #[test]
    fn test_build() {
        let mut times = Times::new();
        let builder = PlaylistBuilder::new()
            .min_size(3)
            .max_size(5)
            .fresh_retention(times.fresh_retention);

        let pl = builder
            .build(&album(vec![
                times.old("old-a"),
                times.old("old-b"),
                times.fresh("new-a"),
                times.fresh("new-b"),
            ]))
            .unwrap();
        // * Result must not exceed min_size with old items
        // * Items must be sorted by timestamp
        // * Fresh items must be preferred over old items
        // * Old items are selected randomly
        let got = names(pl);
        assert_eq!(3, got.len());
        assert_eq!(vec!["new-a", "new-b"], &got[0..=1]);
        assert!(got[2] == "old-a" || got[2] == "old-b");

        let pl = builder
            .build(&album(vec![times.fresh("new-a"), times.fresh("new-b")]))
            .unwrap();
        // * If album contains items less than min_size the result must contain just them once
        assert_eq!(vec!["new-a", "new-b"], names(pl));

        let newest = times.fresh("new-a");
        let pl = builder
            .build(&album(vec![
                times.fresh("new-b"),
                times.fresh("new-c"),
                times.fresh("new-d"),
                times.fresh("new-e"),
                times.fresh("new-f"),
                newest,
            ]))
            .unwrap();
        // * If there are fresh items more than min_size the result contain them up to max_size
        // * In case the first 5 items are selected even if there are much newer items in below
        assert_eq!(vec!["new-b", "new-c", "new-d", "new-e", "new-f"], names(pl));

        let album = album(vec![
            times.old("old-a"),
            times.old("new-b"),
            times.old("new-c"),
        ]);
        let pivot = builder.build(&album).unwrap();
        let mut all_same = true;
        for _ in 0..10 {
            let pl = builder.build(&album).unwrap();
            if pivot != pl {
                all_same = false;
                break;
            }
        }
        assert!(!all_same);
    }
}
