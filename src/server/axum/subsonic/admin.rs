//! Users and the library scan.

use crate::app::scanner::State;

use super::browse::music_folders;
use super::ctx::Ctx;
use super::params::Params;
use super::response::{Element, SubsonicError};

async fn user_element(ctx: &Ctx, username: &str, is_admin: bool) -> Element {
	let folders: Vec<i64> = music_folders(ctx)
		.await
		.into_iter()
		.map(|(id, _)| id)
		.collect();
	Element::new()
		.attr("username", username)
		.attr("scrobblingEnabled", true)
		.attr("adminRole", is_admin)
		.attr("settingsRole", is_admin)
		.attr("downloadRole", true)
		.attr("uploadRole", false)
		.attr("playlistRole", true)
		.attr("coverArtRole", false)
		.attr("commentRole", false)
		.attr("podcastRole", false)
		.attr("streamRole", true)
		.attr("jukeboxRole", false)
		.attr("shareRole", false)
		.attr("videoConversionRole", false)
		.scalars("folder", folders)
}

pub async fn get_user(ctx: &Ctx, params: &Params) -> Result<Element, SubsonicError> {
	let username = params.required("username")?;
	if username != ctx.user && !ctx.is_admin().await {
		return Err(SubsonicError::not_authorized());
	}
	let user = ctx
		.config
		.get_user(username)
		.await
		.map_err(|_| SubsonicError::not_found())?;
	Ok(Element::new().object("user", user_element(ctx, &user.name, user.is_admin()).await))
}

pub async fn get_users(ctx: &Ctx, _params: &Params) -> Result<Element, SubsonicError> {
	if !ctx.is_admin().await {
		return Err(SubsonicError::not_authorized());
	}
	let mut users = ctx.config.get_users().await;
	users.sort_by(|a, b| a.name.cmp(&b.name));
	let mut elements = Vec::with_capacity(users.len());
	for user in &users {
		elements.push(user_element(ctx, &user.name, user.is_admin()).await);
	}
	Ok(Element::new().object("users", Element::new().list("user", elements)))
}

async fn scan_status(ctx: &Ctx) -> Element {
	let status = ctx.scanner.get_status().await;
	Element::new().object(
		"scanStatus",
		Element::new()
			.attr("scanning", matches!(status.state, State::InProgress))
			.attr("count", status.num_songs_indexed),
	)
}

pub async fn get_scan_status(ctx: &Ctx, _params: &Params) -> Result<Element, SubsonicError> {
	Ok(scan_status(ctx).await)
}

pub async fn start_scan(ctx: &Ctx, _params: &Params) -> Result<Element, SubsonicError> {
	if !ctx.is_admin().await {
		return Err(SubsonicError::not_authorized());
	}
	ctx.scanner.try_trigger_scan();
	Ok(scan_status(ctx).await)
}
