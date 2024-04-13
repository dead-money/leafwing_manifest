use std::error::Error;

use bevy::{
    asset::Asset,
    ecs::{system::Resource, world::World},
};
use thiserror::Error;

use crate::identifier::Id;

/// A manifest is a collection of ready-to-use game objects,
/// which are loaded from disk and stored in the ECS as a resource.
///
/// The data on the disk is stored in a serialization-friendly format: [`Manifest::RawManifest`].
/// These types have simple structures and are easy to read and write.
/// Once these are all loaded, they are processed into the final manifest.
///
/// With a manifest in hand, game objects are looked up by their unique [`Id`],
/// returning an object of type [`Manifest::Item`].
///
/// Types that implement [`Manifest`] are generally simple hashmap data structures, mapping `Id<Item>` to `Item`.
/// However, if looking up objects by their name is required, the [`NamedManifest`] trait can be added,
/// and strings are used as the key instead of `Id<Item>`.
/// The `Id` can then be quickly generated, using the built-in [`Id::from_name`] stable hash method.
///
/// The elements of the manifest should generally be treated as immutable, as they are shared across the game,
/// and represent the "canonical" version of the game objects.
/// However, mutable accessors are provided, allowing for the runtime addition of new game objects,
/// as might be used for things like user-generated content or modding.
pub trait Manifest: Sized + Resource {
    /// The raw data type that is loaded from disk.
    ///
    /// This type may be `Self`, if no further processing is required.
    ///
    /// While the raw manifest *can* be stored on disk as a dictionary/map of items,
    /// keyed by either their name or `Id`, it is generally more efficient (and easier to hand-author)
    /// if it is instead stored as a simple flat list.
    type RawManifest: Asset;

    /// The raw data type that is stored in the manifest.
    type RawItem;

    /// The type of the game object stored in the manifest.
    ///
    /// These are commonly [`Bundle`](bevy::ecs::bundle::Bundle) types, allowing you to directly spawn them into the [`World`](bevy::ecs::world::World).
    /// If you wish to store [`Handles`](bevy::asset::Handle) to other assets (such as textures, sprites or sounds),
    /// starting the asset loading process for those assets in [`from_raw_manifest`](Manifest::from_raw_manifest) works very well!
    type Item: TryFrom<Self::RawItem, Error = Self::ConversionError>;

    /// The error type that can occur when converting raw manifests into a manifest.
    ///
    /// When implementing this trait for a manifest without any conversion steps,
    /// this type can be set to [`Infallible`](std::convert::Infallible).
    ///
    /// If you want to reprocess the manifest,
    /// consider returning the raw manifest in the error type.
    type ConversionError: Error;

    /// Converts a raw manifest into the corresponding manifest.
    ///
    /// This is an inherently fallible operation, as the raw data may be malformed or invalid.
    ///
    /// If you wish to reference assets in the [`Item`](Manifest::Item) type, you can start the asset loading process here,
    /// and store a strong reference to the [`Handle`](bevy::asset::Handle) in the item.
    ///
    /// If you need access to data from *other* manifests, you can use the [`World`](bevy::ecs::world::World) to look them up as resources.
    /// This is useful for cross-referencing data between manifests.
    /// Use ordinary system ordering to ensure that the required manifests are loaded first:
    /// the system that calls this method is [`process_manifest::<M>`](crate::plugin::process_manifest), run in the [`PreUpdate`](bevy::prelude::PreUpdate) schedule.
    ///
    /// This method is commonly implemented using the [`TryFrom`] trait between [`Self::RawItem`](Manifest::RawItem) and [`Self::Item`](Manifest::Item).
    /// By iterating over the items in the raw manifest, you can convert them into the final item type one at a time.
    fn from_raw_manifest(
        raw_manifest: Self::RawManifest,
        world: &mut World,
    ) -> Result<Self, Self::ConversionError>;

    /// Converts and then inserts a raw item into the manifest.
    ///
    /// This is a convenience method that combines the conversion and insertion steps.
    fn insert_raw_item(
        &mut self,
        raw_item: Self::RawItem,
    ) -> Result<Id<Self::Item>, ManifestModificationError<Self>> {
        let conversion_result = TryFrom::<Self::RawItem>::try_from(raw_item);

        match conversion_result {
            Ok(item) => self.insert(item),
            Err(e) => Err(ManifestModificationError::ConversionFailed(e)),
        }
    }

    /// Inserts a new item into the manifest.
    ///
    /// The item is given a unique identifier, which is returned.
    ///
    /// The [`Id`] typically used as a key here should be generated via the [`Id::from_name`] method,
    /// which hashes the name (fetched from a field on the raw item) into a collision-resistant identifier.
    ///
    /// If a duplicate entry is found, you should return [`Err(ManifestModificationError::DuplicateName(name))`](ManifestModificationError::DuplicateName).
    fn insert(
        &mut self,
        item: Self::Item,
    ) -> Result<Id<Self::Item>, ManifestModificationError<Self>>;

    /// Removes an item from the manifest.
    ///
    /// The item removed is returned, if it was found.
    fn remove(
        &mut self,
        id: &Id<Self::Item>,
    ) -> Result<Id<Self::Item>, ManifestModificationError<Self>>;

    /// Gets an item from the manifest by its unique identifier.
    ///
    /// Returns [`None`] if no item with the given ID is found.
    fn get(&self, id: &Id<Self::Item>) -> Option<&Self::Item>;

    /// Gets a mutable reference to an item from the manifest by its unique identifier.
    ///
    /// Returns [`None`] if no item with the given ID is found.
    fn get_mut(&mut self, id: &Id<Self::Item>) -> Option<&mut Self::Item>;
}

/// A trait for manifests that have named items.
///
/// Naming items can be useful for quick-prototyping, or for hybrid code and data-driven workflows.
///
/// However, named items can be less efficient than using [`Id`]s, as they require string lookups and an additional string-based mapping.
/// As a result, the methods of this trait have been split from the main [`Manifest`] trait,
/// and should be used with deliberation.
pub trait NamedManifest: Manifest {
    /// Gets the unique identifier of an item by its name.
    ///
    /// Returns [`None`] if no item with the given name is found.
    fn id_of(&self, name: &str) -> Option<Id<Self::Item>>;

    /// Removes an item from the manifest by name.
    ///
    /// The item removed is returned, if it was found.
    fn remove_by_name(
        &mut self,
        name: &str,
    ) -> Result<Id<Self::Item>, ManifestModificationError<Self>> {
        self.id_of(name)
            .ok_or_else(|| ManifestModificationError::NameNotFound(name.to_string()))
            .and_then(|id| self.remove(&id))
    }

    /// Gets an item from the manifest by its name.
    ///
    /// Returns [`None`] if no item with the given name is found.
    fn get_by_name(&self, name: &str) -> Option<&Self::Item> {
        self.id_of(name).and_then(|id| self.get(&id))
    }

    /// Gets a mutable reference to an item from the manifest by its name.
    ///
    /// Returns [`None`] if no item with the given name is found.
    fn get_mut_by_name(&mut self, name: &str) -> Option<&mut Self::Item> {
        self.id_of(name).and_then(move |id| self.get_mut(&id))
    }
}

/// An error that can occur when modifying a manifest.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ManifestModificationError<M: Manifest> {
    /// The name of the item is already in use.
    #[error("The name {} is already in use.", _0)]
    DuplicateName(String),
    /// The raw item could not be converted.
    ///
    /// The error that occurred during the conversion is included.
    #[error("The raw item could not be converted.")]
    ConversionFailed(M::ConversionError),
    /// The item with the given ID was not found.
    #[error("The item with ID {:?} was not found.", _0)]
    NotFound(Id<M::Item>),
    /// The item with the given name was not found.
    #[error("No item with the name {} was found.", _0)]
    NameNotFound(String),
}
