use std::any::{type_name, TypeId};
use std::path::PathBuf;

use bevy::app::{App, Plugin, PreUpdate, Update};
use bevy::asset::{AssetApp, AssetLoadFailedEvent, AssetServer, Assets, LoadState, UntypedHandle};
use bevy::ecs::prelude::*;
use bevy::ecs::system::SystemState;
use bevy::log::error_once;
use bevy::utils::HashMap;

use crate::asset_state::AssetLoadingState;
use crate::manifest::Manifest;

/// A plugin for loading assets from a [`Manifest`].
///
/// This plugin will add the required state to your app (starting in [`AppLoadingState::LOADING`]),
/// and set up the required systems to progress through the asset loading process and parse any added manifests.
///
/// Note that manifests must be added to the app manually, using the [`app.register_manifest`](crate::AppExt::register_manifest) method.
/// This plugin **must** be added before manifests are registered.
///
/// While [`register_manifest`](crate::AppExt::register_manifest) must be called for each manifest type you wish to use,
/// this plugin should only be added a single time.
///
/// This plugin is intenionally optional: if you have more complex asset loading requirements, take a look at the systems in this plugin and either add or reimplement them as needed.
#[derive(Debug, Default)]
pub struct ManifestPlugin<S: States> {
    _phantom: std::marker::PhantomData<S>,
}

impl<S: AssetLoadingState> Plugin for ManifestPlugin<S> {
    fn build(&self, app: &mut App) {
        app.insert_state(S::LOADING)
            .init_resource::<RawManifestTracker>()
            .add_systems(
                Update,
                check_if_manifests_have_loaded::<S>.run_if(in_state(S::LOADING)),
            )
            .add_systems(
                Update,
                check_if_manifests_are_processed::<S>.run_if(in_state(S::PROCESSING)),
            );
    }
}

/// An extension trait for registering manifests with an app.
pub trait AppExt {
    /// Registers a manifest with the app, preparing it for loading and parsing.
    ///
    /// The final manifest type must implement [`Manifest`], while the raw manifest type must implement [`Asset`](bevy::asset::Asset).
    /// This must be called for each type of manifest you wish to load.
    fn register_manifest<M: Manifest>(&mut self, path: impl Into<PathBuf>) -> &mut Self;
}

impl AppExt for App {
    /// Registers the manifest `M`.
    ///
    /// By default, the path root is the `assets` folder, just like all Bevy assets.
    fn register_manifest<M: Manifest>(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.init_asset::<M::RawManifest>()
            .add_systems(
                Update,
                report_failed_raw_manifest_loading::<M>
                    .run_if(on_event::<AssetLoadFailedEvent<M::RawManifest>>()),
            )
            .add_systems(
                PreUpdate,
                process_manifest::<M>.run_if(not(resource_exists::<M>)),
            );

        self.world
            .resource_scope(|world, mut asset_server: Mut<AssetServer>| {
                let mut manifest_tracker = world.resource_mut::<RawManifestTracker>();
                manifest_tracker.register::<M>(path, asset_server.as_mut());
            });

        self
    }
}

/// Keeps track of the raw manifests that need to be loaded, and their loading progress.
#[derive(Resource, Debug, Default)]
pub struct RawManifestTracker {
    raw_manifests: HashMap<TypeId, RawManifestStatus>,
    processing_status: ProcessingStatus,
}

/// The current processing status of the raw manifests into manifests.
#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub enum ProcessingStatus {
    /// The raw manifests are still being processed.
    #[default]
    Processing,
    /// The raw manifests have been processed and are ready to use.
    Ready,
    /// The raw manifests could not be properly processed.
    Failed,
}

/// Information about the loading status of a raw manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawManifestStatus {
    /// The path to the manifest file.
    pub path: PathBuf,
    /// A strong handle to the raw manifest.
    pub handle: UntypedHandle,
    /// The computed loading state of the raw manifest.
    pub load_state: LoadState,
}

impl RawManifestTracker {
    /// Registers a manifest to be loaded.
    ///
    /// This must be done before [`AssetLoadingState::LOADING`] is complete.
    pub fn register<M: Manifest>(
        &mut self,
        path: impl Into<PathBuf>,
        asset_server: &mut AssetServer,
    ) {
        let path: PathBuf = path.into();

        let handle: UntypedHandle = asset_server.load::<M::RawManifest>(path.clone()).untyped();
        let type_id = std::any::TypeId::of::<M>();

        self.raw_manifests.insert(
            type_id,
            RawManifestStatus {
                path: path.clone(),
                handle,
                load_state: LoadState::Loading,
            },
        );
    }

    /// Returns the load state and other metadata for the given manifest.
    pub fn status<M: Manifest>(&self) -> Option<&RawManifestStatus> {
        self.raw_manifests.get(&std::any::TypeId::of::<M>())
    }

    /// Iterates over all registered raw manifests.
    pub fn iter(&self) -> impl Iterator<Item = (&TypeId, &RawManifestStatus)> {
        self.raw_manifests.iter()
    }

    /// Updates the load state of all registered raw manifests.
    pub fn update_load_states(&mut self, asset_server: &AssetServer) {
        for status in self.raw_manifests.values_mut() {
            status.load_state = asset_server
                .get_load_state(status.handle.clone_weak())
                .unwrap_or(LoadState::Failed);
        }
    }

    /// Returns true if all registered raw manifests have loaded.
    pub fn all_manifests_loaded(&mut self, asset_server: &AssetServer) -> bool {
        self.update_load_states(asset_server);

        self.raw_manifests
            .values()
            .all(|status| status.load_state == LoadState::Loaded)
    }

    /// Returns true if any registered raw manifests have failed to load.
    pub fn any_manifests_failed(&mut self, asset_server: &AssetServer) -> bool {
        self.update_load_states(asset_server);

        self.raw_manifests
            .values()
            .any(|status| status.load_state == LoadState::Failed)
    }

    /// Returns the [`ProcessingStatus`] of the raw manifests.
    pub fn processing_status(&self) -> ProcessingStatus {
        self.processing_status
    }

    /// Sets the [`ProcessingStatus`] of the raw manifests.
    pub fn set_processing_status(&mut self, status: ProcessingStatus) {
        self.processing_status = status;
    }
}

/// Checks if all registered assets have loaded,
/// and progresses to [`AssetLoadingState::PROCESSING`] if they have.
///
/// If any assets have failed to load, the state will be set to [`AssetLoadingState::FAILED`].
pub fn check_if_manifests_have_loaded<S: AssetLoadingState>(
    asset_server: Res<AssetServer>,
    mut raw_manifest_tracker: ResMut<RawManifestTracker>,
    mut next_state: ResMut<NextState<S>>,
) {
    if raw_manifest_tracker.any_manifests_failed(asset_server.as_ref()) {
        next_state.set(S::FAILED);
    } else if raw_manifest_tracker.all_manifests_loaded(asset_server.as_ref()) {
        next_state.set(S::PROCESSING);
    }
}

/// Checks if all manifests are processed, and progresses to [`AssetLoadingState::READY`] if they are.
/// If any manifests have failed to process, the state will be set to [`AssetLoadingState::FAILED`].
pub fn check_if_manifests_are_processed<S: AssetLoadingState>(
    raw_manifest_tracker: Res<RawManifestTracker>,
    mut next_state: ResMut<NextState<S>>,
) {
    if raw_manifest_tracker.processing_status() == ProcessingStatus::Failed {
        next_state.set(S::FAILED);
    } else if raw_manifest_tracker.processing_status() == ProcessingStatus::Ready {
        next_state.set(S::READY);
    }
}

/// Watches for and reports failed raw manifest loading events.
///
/// This generic system is currently required as [`LoadState::Failed`] does not contain the error that caused the failure.
///
/// See [bevy#12667](https://github.com/bevyengine/bevy/issues/12667) for more information.0
pub fn report_failed_raw_manifest_loading<M: Manifest>(
    mut events: EventReader<AssetLoadFailedEvent<M::RawManifest>>,
) {
    for event in events.read() {
        error_once!(
            "Failed to load asset at {} due to {:?}",
            event.path,
            event.error
        );
    }
}

/// A system which processes a raw manifest into a completed [`Manifest`],
/// and then stores the manifest as a [`Resource`] in the [`World`].
///
/// The raw manifest will be removed from the [`AssetServer`] as part of creation.
pub fn process_manifest<M: Manifest>(
    world: &mut World,
    system_state: &mut SystemState<(Res<RawManifestTracker>, ResMut<Assets<M::RawManifest>>)>,
) {
    let (raw_manifest_tracker, mut assets) = system_state.get_mut(world);
    let Some(status) = raw_manifest_tracker.status::<M>() else {
        error_once!(
            "The status of the raw manifest corresponding to the manifest type {} was not found.",
            type_name::<M>()
        );
        return;
    };
    let typed_handle = status.handle.clone_weak().typed::<M::RawManifest>();
    let maybe_raw_manifest = assets.remove(typed_handle);

    let raw_manifest = match maybe_raw_manifest {
        Some(raw_manifest) => raw_manifest,
        None => {
            error_once!(
                "Failed to get raw manifest for manifest type {} from the asset server.",
                type_name::<M>()
            );
            return;
        }
    };

    match M::from_raw_manifest(raw_manifest, world) {
        Ok(manifest) => {
            world.insert_resource(manifest);
            // We can't just use a ResMut above, since we need to drop the borrow before we can construct the manifest.
            let mut raw_manifest_tracker = world.resource_mut::<RawManifestTracker>();
            raw_manifest_tracker.set_processing_status(ProcessingStatus::Ready);
        }
        Err(err) => {
            error_once!("Failed to process manifest: {:?}", err);
            let mut raw_manifest_tracker = world.resource_mut::<RawManifestTracker>();
            raw_manifest_tracker.set_processing_status(ProcessingStatus::Failed);
        }
    }
}
