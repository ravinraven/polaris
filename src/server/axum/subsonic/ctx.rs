//! What an endpoint needs to do its work: the app's managers, and who is asking.

use crate::app::{
	config, index, playlist, scanner,
	state::{self, ObjectKind},
	thumbnail, App, Error,
};

use super::auth::{Authenticated, Mechanism};
use super::response::SubsonicError;

#[derive(Clone)]
pub struct Ctx {
	pub config: config::Manager,
	pub state: state::Manager,
	pub index: index::Manager,
	pub playlists: playlist::Manager,
	pub thumbnails: thumbnail::Manager,
	pub scanner: scanner::Scanner,
	pub user: String,
	pub mechanism: Mechanism,
}

pub fn internal(_: Error) -> SubsonicError {
	SubsonicError::generic("Internal error")
}

impl Ctx {
	pub fn new(app: &App, user: &Authenticated) -> Self {
		Self {
			config: app.config_manager.clone(),
			state: app.state_manager.clone(),
			index: app.index_manager.clone(),
			playlists: app.playlist_manager.clone(),
			thumbnails: app.thumbnail_manager.clone(),
			scanner: app.scanner.clone(),
			user: user.username.clone(),
			mechanism: user.mechanism,
		}
	}

	pub async fn is_admin(&self) -> bool {
		self.config
			.get_user(&self.user)
			.await
			.map(|u| u.is_admin())
			.unwrap_or(false)
	}

	/// The kind and natural key an ID refers to. Unknown IDs are "not found".
	pub async fn resolve(&self, id: &str) -> Result<(ObjectKind, String), SubsonicError> {
		self.state
			.resolve_id(id)
			.await
			.map_err(internal)?
			.ok_or_else(SubsonicError::not_found)
	}

	/// Like [`Ctx::resolve`], but the ID must refer to the given kind of object.
	pub async fn resolve_as(&self, id: &str, kind: ObjectKind) -> Result<String, SubsonicError> {
		let (actual, key) = self.resolve(id).await?;
		if actual == kind {
			Ok(key)
		} else {
			Err(SubsonicError::not_found())
		}
	}
}
