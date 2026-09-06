// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

mod annexb;
mod capture;
pub mod encode;
mod input;
mod net;
pub mod shared;

use std::net::{TcpListener, ToSocketAddrs, UdpSocket};
use std::thread::JoinHandle;

use godot::builtin::{Callable, GString};
use godot::global::Error;
use godot::prelude::*;

use crate::shared::{SharedState, StreamConfig, MAX_TARGET_FPS};

struct GameStreamExtension;

#[gdextension]
unsafe impl ExtensionLibrary for GameStreamExtension {}

struct StreamRuntime {
    shared: std::sync::Arc<SharedState>,
    post_draw: Callable,
    draw_watchdog: Option<Callable>,
    encode_thread: Option<JoinHandle<()>>,
    net_thread: Option<JoinHandle<()>>,
}

impl StreamRuntime {
    fn shutdown(mut self) {
        self.shared.request_stop();
        capture::disconnect_post_draw(&self.post_draw);
        if let Some(callable) = &self.draw_watchdog {
            capture::disconnect_draw_watchdog(callable);
        }
        capture::request_gpu_cleanup(&self.shared);
        if let Some(handle) = self.encode_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.net_thread.take() {
            let _ = handle.join();
        }
    }
}

#[derive(GodotClass)]
#[class(init, singleton)]
pub struct GameStream {
    session: Option<StreamRuntime>,
    base: Base<Object>,
}

#[godot_api]
impl GameStream {
    #[func]
    fn listen(&mut self, bind: GString, port: i32) -> Error {
        self.listen_config(bind, port, StreamConfig::default(), None, true)
    }

    #[func]
    fn listen_with_options(
        &mut self,
        bind: GString,
        port: i32,
        width: i32,
        height: i32,
        fps: i32,
    ) -> Error {
        let Some(config) = (width >= 2 && height >= 2 && fps >= 1)
            .then(|| StreamConfig::new(width as u32, height as u32, fps as u32))
            .flatten()
        else {
            godot_error!(
                "game_stream: invalid stream options {width}x{height}@{fps}; fps must be 1..={MAX_TARGET_FPS}"
            );
            return Error::ERR_INVALID_PARAMETER;
        };
        self.listen_config(bind, port, config, None, true)
    }

    /// Explicit settings for the standalone addon. Environment variables do
    /// not override these settings, so each project's inspector is authoritative.
    #[func]
    #[allow(clippy::too_many_arguments)]
    fn listen_configured(
        &mut self,
        bind: GString,
        port: i32,
        width: i32,
        height: i32,
        fps: i32,
        codec: GString,
        allow_input: bool,
    ) -> Error {
        let Some(codec) = shared::StreamCodec::from_env_value(&codec.to_string()) else {
            return Error::ERR_INVALID_PARAMETER;
        };
        let Some(config) = (width >= 2 && height >= 2 && fps >= 1)
            .then(|| StreamConfig::with_codec(width as u32, height as u32, fps as u32, codec))
            .flatten()
        else {
            return Error::ERR_INVALID_PARAMETER;
        };
        self.listen_config(bind, port, config, Some(allow_input), false)
    }

    #[func]
    fn stop(&mut self) {
        if let Some(session) = self.session.take() {
            godot_print!("game_stream: stopping");
            session.shutdown();
        }
    }

    #[func]
    fn is_listening(&self) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session
                .shared
                .running
                .load(std::sync::atomic::Ordering::SeqCst)
        })
    }

    #[func]
    fn stats(&self) -> VarDictionary {
        match &self.session {
            Some(session) => session.shared.snapshot(),
            None => VarDictionary::new(),
        }
    }
}

impl GameStream {
    fn listen_config(
        &mut self,
        bind: GString,
        port: i32,
        mut config: StreamConfig,
        input_override: Option<bool>,
        use_environment_codec: bool,
    ) -> Error {
        self.stop();

        if !(1..=65535).contains(&port) {
            godot_error!("game_stream: invalid port {port}");
            return Error::ERR_INVALID_PARAMETER;
        }
        if use_environment_codec {
            if let Some(codec) = codec_from_env() {
                godot_print!("game_stream: stream codec override: {}", codec.as_str());
                config.codec = codec;
            }
        }
        if !capture::rendering_device_available() {
            godot_warn!(
                "game_stream: RenderingDevice unavailable (Compatibility/headless). listen skipped."
            );
            return Error::ERR_UNAVAILABLE;
        }
        let Some(viewport) = capture::root_viewport_rid() else {
            godot_warn!("game_stream: root viewport RID unavailable");
            return Error::ERR_UNAVAILABLE;
        };

        let bind = bind.to_string();
        let bind = if bind.trim().is_empty() {
            "127.0.0.1".to_string()
        } else {
            bind
        };
        let (transport, transport_description) = if let Some(target) = bind.strip_prefix("udp://") {
            if target.trim().is_empty() {
                godot_error!("game_stream: UDP destination host is empty");
                return Error::ERR_INVALID_PARAMETER;
            }
            let destination = match (target, port as u16).to_socket_addrs() {
                Ok(mut addresses) => {
                    match addresses.next() {
                        Some(address) => address,
                        None => {
                            godot_error!("game_stream: UDP destination {target}:{port} resolved no addresses");
                            return Error::ERR_CANT_RESOLVE;
                        }
                    }
                }
                Err(error) => {
                    godot_error!(
                        "game_stream: resolve UDP destination {target}:{port} failed: {error}"
                    );
                    return Error::ERR_CANT_RESOLVE;
                }
            };
            let local_bind = if destination.is_ipv4() {
                "0.0.0.0:0"
            } else {
                "[::]:0"
            };
            let socket = match UdpSocket::bind(local_bind) {
                Ok(socket) => socket,
                Err(error) => {
                    godot_error!("game_stream: create UDP sender failed: {error}");
                    return Error::ERR_CANT_OPEN;
                }
            };
            if let Err(error) = socket.set_nonblocking(true) {
                godot_error!("game_stream: configure UDP sender failed: {error}");
                return Error::ERR_CANT_OPEN;
            }
            match socket.local_addr() {
                Ok(address) => godot_print!(
                    "game_stream: UDP sender bound {address}, destination {destination}"
                ),
                Err(error) => godot_warn!("game_stream: UDP local_addr failed: {error}"),
            }
            (
                net::Transport::Udp {
                    socket,
                    destination,
                },
                format!("sending UDP to {destination}"),
            )
        } else {
            let listener = match TcpListener::bind((bind.as_str(), port as u16)) {
                Ok(listener) => listener,
                Err(error) => {
                    godot_error!("game_stream: bind {bind}:{port} failed: {error}");
                    return Error::ERR_CANT_OPEN;
                }
            };
            match listener.local_addr() {
                Ok(addr) => godot_print!("game_stream: bound {addr}"),
                Err(error) => godot_warn!("game_stream: bound but local_addr failed: {error}"),
            }
            (
                net::Transport::Tcp(listener),
                format!("listening TCP on {bind}:{port}"),
            )
        };

        if let Err(error) = encode::init_ffmpeg() {
            godot_error!("game_stream: FFmpeg init failed (an LGPL build is required): {error}");
            return Error::FAILED;
        }

        let input_enabled = input_override.unwrap_or_else(input_enabled_from_env);
        let shared = SharedState::new(viewport, config, input_enabled);
        let encode_shared = std::sync::Arc::clone(&shared);
        let encode_thread = std::thread::Builder::new()
            .name("game_stream_encode".into())
            .spawn(move || {
                if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    encode::run_encoder_thread(encode_shared)
                })) {
                    godot::prelude::godot_error!("game_stream: encode thread panicked: {panic:?}");
                }
            });
        let encode_thread = match encode_thread {
            Ok(handle) => handle,
            Err(error) => {
                godot_error!("game_stream: encode thread spawn failed: {error}");
                shared.request_stop();
                return Error::ERR_CANT_CREATE;
            }
        };

        let net_shared = std::sync::Arc::clone(&shared);
        let net_thread = std::thread::Builder::new()
            .name("game_stream_net".into())
            .spawn(move || {
                if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    net::run_net_thread(net_shared, transport)
                })) {
                    godot::prelude::godot_error!("game_stream: net thread panicked: {panic:?}");
                }
            });
        let net_thread = match net_thread {
            Ok(handle) => handle,
            Err(error) => {
                godot_error!("game_stream: net thread spawn failed: {error}");
                shared.request_stop();
                encode_thread.join().ok();
                return Error::ERR_CANT_CREATE;
            }
        };

        let post_draw = capture::connect_post_draw(&shared);
        let draw_watchdog = capture::connect_draw_watchdog(&shared);
        self.session = Some(StreamRuntime {
            shared,
            post_draw,
            draw_watchdog,
            encode_thread: Some(encode_thread),
            net_thread: Some(net_thread),
        });

        godot_print!(
            "game_stream: {transport_description} (requested {}x{}@{}, requested_codec={}, max {}fps, source-clamped raw elementary stream, reverse input channel {})",
            config.requested_width,
            config.requested_height,
            config.fps,
            config.codec.as_str(),
            MAX_TARGET_FPS,
            if input_enabled {
                "on"
            } else {
                "off (GAME_STREAM_INPUT=0)"
            },
        );
        Error::OK
    }
}

/// Input injection defaults to on whenever the stream itself is enabled; the
/// env var is only a kill switch, so no game-side wiring is required.
fn input_enabled_from_env() -> bool {
    !matches!(
        std::env::var("GAME_STREAM_INPUT")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "off" | "no"
    )
}

/// Optional wire codec override (`GAME_STREAM_CODEC=auto|av1|hevc|h264`), mirroring the
/// other GAME_STREAM_* switches: no game-side wiring required.
fn codec_from_env() -> Option<crate::shared::StreamCodec> {
    let value = std::env::var("GAME_STREAM_CODEC").unwrap_or_default();
    if value.trim().is_empty() {
        return None;
    }
    let codec = crate::shared::StreamCodec::from_env_value(&value);
    if codec.is_none() {
        godot_warn!(
            "game_stream: unknown GAME_STREAM_CODEC {value:?}; expected auto, av1, hevc/h265, or h264"
        );
    }
    codec
}

impl Drop for GameStream {
    fn drop(&mut self) {
        self.stop();
    }
}
