use failure::Fail;
use std::fmt::Debug;
use std::iter::Iterator;
use std::path::Path;
use std::time::SystemTime;

pub trait Error: Fail {
    /// Return if this error is caused by fatal error such as local hardware
    /// glitch or misconfiguration that it is hopeless to keep running the app.
    fn is_fatal(&self) -> bool;
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum MediaType {
    PHOTO,
    VIDEO,
}

pub trait Album {
    type E: Error + 'static;
    type Item: AlbumItem + Eq + Debug + 'static;
    type Items: Iterator<Item = Result<Self::Item, Self::E>>;

    /// Return the list of items as iterator.
    /// Preferrably it obtains items lazily during the call of `#next()`.
    fn items(&self) -> Self::Items;

    /// Prepare specified item for upcoming access.
    ///
    /// This particularly expects operations which may takes long such as
    /// downloading contents from a cloud storage.
    fn prepare_item<P: AsRef<Path>>(&self, item: &Self::Item, path: P) -> Result<(), Self::E>;
}

pub trait AlbumItem: PartialEq + Debug {
    /// Return ID of this item
    fn id(&self) -> &str;

    /// Return the local file path storing this item's content.
    fn path(&self) -> &Path;

    /// Return this item's media type.
    fn media_type(&self) -> MediaType;

    /// Return the creation time of this item.
    fn created_time(&self) -> SystemTime;
}
