//! Connect's remote-only Cube catalog and its request ownership.

use super::{
    catalog_display_name, derive_phone_recovery, launcher_catalog, locally_stored_cubes,
    CoincubeError, CubeResponse, GhostImport, Home, Message, RemoteCube, Task,
};

impl Home {
    /// The signed-in Connect account, when its user record is loaded.
    pub(super) fn connect_account_id(&self) -> Option<u32> {
        self.connect_account
            .authenticated_client()
            .and(self.connect_account.user.as_ref())
            .map(|user| user.id)
    }

    /// A ghost import goes ahead only while the account that listed the
    /// ghost is signed in and still lists it, on the same network.
    pub(super) fn ghost_still_registered(&self, ghost: &GhostImport) -> Result<(), String> {
        let listed = self.remote_cubes.iter().any(|cube| {
            cube.uuid == ghost.uuid && launcher_catalog::ghost_chain(cube) == Some(ghost.network)
        });
        if listed && ghost.account == self.connect_account_id() {
            Ok(())
        } else {
            Err(coincube_ui::localized_format!(
                "Connect no longer lists {} for this account. Cancel, then pick it again from the Launcher.",
                "Connect ya no muestra {} en esta cuenta. Cancela y vuelve a elegirlo desde el Launcher.",
                catalog_display_name(&ghost.name)
            ))
        }
    }

    /// Fetch the full server cube list and keep the ones with no local
    /// counterpart — the "restore from Connect" rows on the Cubes page. Both
    /// the API call and the local settings reads run off the UI thread.
    /// `Task::none()` when signed out; the list is Connect-only.
    ///
    /// Must be re-run whenever the *local* set changes, not just at sign-in.
    /// `Message::Checked` can only prune this list (drop rows that became
    /// local); a Cube deleted locally while its Connect copy was kept becomes
    /// remote-only at that instant, and nothing but a re-fetch can discover it.
    /// Without this it stayed invisible — and so unrestorable — until the next
    /// sign-in.
    ///
    /// Claims a fresh [`remote_cubes_request`](Self::remote_cubes_request)
    /// generation and carries it through to `RemoteCubesLoaded`, so a result
    /// that lands after a newer fetch (or after logout) is discarded rather
    /// than applied. Takes `&mut self` for that reason.
    pub(super) fn fetch_remote_cubes(&mut self) -> Task<Message> {
        // Bump unconditionally, before the signed-out bail: a fetch starting
        // while signed out still means "anything older is superseded", and
        // leaving the generation untouched here would let an in-flight result
        // from a prior session land afterwards.
        let request_id = self.invalidate_remote_cubes();
        let Some(rc_client) = self.connect_account.authenticated_client() else {
            return Task::none();
        };
        let datadir = self.datadir_path.clone();
        Task::perform(
            async move {
                let server_cubes = rc_client.list_cubes().await.map_err(|e| e.to_string())?;

                // Collect local cube UUIDs across all networks
                let mut local_uuids = std::collections::HashSet::new();
                for cube in locally_stored_cubes(&datadir) {
                    local_uuids.insert(cube.id);
                }

                // Every listed Cube over the plan's limit, local or not.
                let connect_off: Vec<String> = server_cubes
                    .iter()
                    .filter(|sc| sc.connect_features_disabled)
                    .map(|sc| sc.uuid.clone())
                    .collect();

                // Keep only server cubes with no local counterpart
                let remote_source: Vec<CubeResponse> = server_cubes
                    .into_iter()
                    .filter(|sc| !local_uuids.contains(&sc.uuid))
                    .collect();

                // Determine restorability per cube. `has_recovery_kit`
                // comes free from `list_cubes()`, but both the seed half
                // (what a password restore needs) and the owner-keychain
                // "phone" envelope summary live on `/recovery-kit/status`,
                // so probe it for *every* remote cube — a phone-only cube
                // has `has_recovery_kit == false` yet is still recoverable
                // via its envelope, so gating the probe on the password
                // kit would hide it. Probes run concurrently (`join_all`)
                // so latency is one round-trip, not N. A flaky probe must
                // not blank the whole list — on any error we log and treat
                // every method as absent (the row still shows, just
                // without a restore icon). `join_all` preserves input
                // order.
                //
                // Duress note: phone availability is read from the
                // `ownerSelf` *status* summary (presence/tier, no
                // ciphertext); the duress gate fires later, at the actual
                // recovery attempt (`OwnerKeychainRestoreStep` →
                // `fetch_owner_recovery_envelope`), which returns neutral
                // "unavailable, try later" copy on a 423 (invariant I3).
                let client_ref = &rc_client;
                let remote_only: Vec<RemoteCube> = iced::futures::future::join_all(
                    remote_source.into_iter().map(|sc| async move {
                        let (has_encrypted_seed, phone_recoverable, phone_full_cube) =
                            match client_ref.get_recovery_kit_status(sc.id).await {
                                Ok(status) => {
                                    let (phone_recoverable, phone_full_cube) =
                                        derive_phone_recovery(status.owner_self.as_ref());
                                    (
                                        status.has_encrypted_seed,
                                        phone_recoverable,
                                        phone_full_cube,
                                    )
                                }
                                // No kit/status yet → nothing recoverable.
                                Err(CoincubeError::NotFound) => (false, false, false),
                                Err(e) => {
                                    log::warn!(
                                        "[LAUNCHER] Recovery Kit status probe \
                                         failed for \"{}\": {}",
                                        sc.name,
                                        e
                                    );
                                    (false, false, false)
                                }
                            };
                        RemoteCube {
                            id: sc.id,
                            uuid: sc.uuid,
                            name: sc.name,
                            network: sc.network,
                            has_recovery_kit: sc.has_recovery_kit,
                            has_encrypted_seed,
                            phone_recoverable,
                            phone_full_cube,
                            encryption_pubkey: sc.encryption_pubkey,
                            has_vault: sc.has_vault,
                        }
                    }),
                )
                .await;

                Ok((remote_only, connect_off))
            },
            move |result: Result<(Vec<RemoteCube>, Vec<String>), String>| {
                let (result, connect_off) = match result {
                    Ok((remote_only, connect_off)) => (Ok(remote_only), connect_off),
                    Err(e) => (Err(e), Vec::new()),
                };
                Message::RemoteCubesLoaded {
                    request_id,
                    result,
                    connect_off,
                }
            },
        )
    }

    /// Supersede every in-flight remote-cube fetch and return the new
    /// generation. Callers that are *starting* a fetch keep the returned id;
    /// callers that only need to discard (logout) ignore it.
    ///
    /// Logout must do this. It clears `remote_cubes`, but a fetch issued for
    /// the previous account is still running and its `RemoteCubesLoaded` would
    /// otherwise repopulate the list — putting the prior account's Cubes back
    /// on screen after sign-out. The neighbouring `recover_vault` reset and
    /// `recovery_method_modal` drop guard the same hazard for their surfaces.
    pub(super) fn invalidate_remote_cubes(&mut self) -> u64 {
        self.remote_cubes_request = self.remote_cubes_request.wrapping_add(1);
        self.remote_cubes_request
    }

    pub(super) fn apply_remote_cubes(
        &mut self,
        request_id: u64,
        result: Result<Vec<RemoteCube>, String>,
        connect_off: Vec<String>,
    ) -> Task<Message> {
        // Superseded by a newer fetch, or invalidated by logout. Drop
        // it: applying it would either roll the list back to an older
        // server view or restore the previous account's Cubes.
        if request_id != self.remote_cubes_request {
            log::debug!(
                "[LAUNCHER] Discarding stale remote-cube fetch #{} (current #{})",
                request_id,
                self.remote_cubes_request
            );
            return Task::none();
        }
        match result {
            Ok(remote_only) => {
                self.remote_cubes = remote_only;
                self.connect_off_cubes = connect_off.into_iter().collect();
            }
            Err(e) => {
                log::warn!("[LAUNCHER] Failed to fetch remote cubes: {e}");
            }
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use httpmock::{Method::GET, MockServer};
    use iced_runtime::futures::futures::StreamExt;
    use serde_json::json;

    fn home(dir: &std::path::Path, url: String) -> Home {
        let datadir = CoincubeDirectory::new(dir.to_path_buf());
        for chain in ChainId::ALL {
            let network_dir = datadir.network_directory(chain);
            std::fs::create_dir_all(network_dir.path()).unwrap();
            let cubes = if chain == ChainId::BitcoinBlake2b {
                vec![CubeSettings::new_with_raw_id(
                    "local".into(),
                    "Local".into(),
                    chain,
                )]
            } else {
                vec![]
            };
            std::fs::write(
                network_dir.path().join("settings.json"),
                serde_json::to_vec(&settings::Settings {
                    cubes,
                    ..Default::default()
                })
                .unwrap(),
            )
            .unwrap();
        }
        let mut home = Home::new(datadir, Some(Network::Bitcoin)).0;
        home.connect_account.client = CoincubeClient::for_test(url);
        home.connect_account.user = Some(crate::services::coincube::User {
            id: 7,
            email: "owner@example.invalid".into(),
            email_verified: Some(true),
        });
        home
    }

    fn cube(id: u64, uuid: &str) -> serde_json::Value {
        json!({"id": id, "uuid": uuid, "name": uuid, "network":"mainnet", "status":"active"})
    }

    async fn outputs(task: Task<Message>) -> Vec<Message> {
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return vec![];
        };
        stream
            .filter_map(|action| async move {
                match action {
                    iced_runtime::Action::Output(message) => Some(message),
                    _ => None,
                }
            })
            .collect()
            .await
    }

    fn cached() -> RemoteCube {
        RemoteCube {
            id: 9,
            uuid: "cached".into(),
            name: "Cached".into(),
            network: "mainnet".into(),
            has_recovery_kit: false,
            has_encrypted_seed: false,
            phone_recoverable: false,
            phone_full_cube: false,
            encryption_pubkey: None,
            has_vault: None,
        }
    }

    #[tokio::test]
    async fn fetch_filters_all_local_chains_and_keeps_phone_only_and_failed_probe_rows() {
        let server = MockServer::start();
        let list = server.mock(|when, then| {
            when.method(GET).path("/api/v1/connect/cubes");
            then.status(200).json_body(json!({"data": [cube(1,"local"), cube(2,"phone"), cube(3,"missing"), cube(4,"offline")]}));
        });
        let local = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/connect/cubes/1/recovery-kit/status");
            then.status(500);
        });
        let phone = server.mock(|when, then| {
            when.method(GET).path("/api/v1/connect/cubes/2/recovery-kit/status");
            then.status(200).json_body(json!({"data": {"hasRecoveryKit": false, "hasEncryptedSeed": false,
                "hasEncryptedWalletDescriptor": false, "encryptionScheme":"aes-256-gcm",
                "ownerSelf": {"hasRecipient":true,"tier":"full","envelopeKinds":["seed","descriptor"]}}}));
        });
        let missing = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/connect/cubes/3/recovery-kit/status");
            then.status(404);
        });
        let offline = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/connect/cubes/4/recovery-kit/status");
            then.status(500);
        });
        let dir = tempfile::tempdir().unwrap();
        let mut home = home(dir.path(), server.base_url());
        for message in outputs(home.fetch_remote_cubes()).await {
            drop(home.update(message));
        }
        list.assert();
        local.assert_hits(0);
        phone.assert();
        missing.assert();
        offline.assert();
        assert_eq!(
            home.remote_cubes
                .iter()
                .map(|cube| cube.uuid.as_str())
                .collect::<Vec<_>>(),
            ["phone", "missing", "offline"]
        );
        assert!(!home.remote_cubes[0].has_recovery_kit);
        assert!(home.remote_cubes[0].phone_recoverable && home.remote_cubes[0].phone_full_cube);
        for cube in &home.remote_cubes[1..] {
            assert!(!cube.has_encrypted_seed && !cube.phone_recoverable && !cube.phone_full_cube);
        }
    }

    #[tokio::test]
    async fn signed_out_fetch_supersedes_a_real_inflight_response() {
        let server = MockServer::start();
        let list = server.mock(|when, then| {
            when.method(GET).path("/api/v1/connect/cubes");
            then.status(200)
                .json_body(json!({"data":[cube(3,"old-account")]}));
        });
        let status = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/connect/cubes/3/recovery-kit/status");
            then.status(404);
        });
        let dir = tempfile::tempdir().unwrap();
        let mut home = home(dir.path(), server.base_url());
        let prior = home.fetch_remote_cubes();
        let generation = home.remote_cubes_request;
        home.connect_account.user = None;
        assert!(outputs(home.fetch_remote_cubes()).await.is_empty());
        assert_ne!(generation, home.remote_cubes_request);
        for message in outputs(prior).await {
            drop(home.update(message));
        }
        list.assert();
        status.assert();
        assert!(home.remote_cubes.is_empty());
    }

    #[tokio::test]
    async fn failed_list_request_preserves_the_previous_catalog() {
        let server = MockServer::start();
        let list = server.mock(|when, then| {
            when.method(GET).path("/api/v1/connect/cubes");
            then.status(503);
        });
        let dir = tempfile::tempdir().unwrap();
        let mut home = home(dir.path(), server.base_url());
        home.remote_cubes = vec![cached()];
        for message in outputs(home.fetch_remote_cubes()).await {
            drop(home.update(message));
        }
        list.assert();
        assert_eq!(home.remote_cubes.len(), 1);
        assert_eq!(home.remote_cubes[0].uuid, "cached");
    }
}
