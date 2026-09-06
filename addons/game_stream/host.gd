# SPDX-License-Identifier: LGPL-2.1-or-later
# Copyright (C) 2026 yearsyan and contributors
extends Node
## Streams the root viewport. Add one host per project.
## Requires a built native extension and a RenderingDevice renderer.

@export var auto_start: bool = false
@export var bind_address: String = "127.0.0.1"
@export_range(1, 65535) var port: int = 20831
@export_range(2, 16384, 2) var width: int = 1280
@export_range(2, 16384, 2) var height: int = 720
@export_range(1, 120) var fps: int = 60
@export_enum("h264", "hevc", "av1", "auto") var codec: String = "h264"
@export var allow_input: bool = true

var _owns_stream: bool = false


func _ready() -> void:
	if auto_start and not Engine.is_editor_hint():
		# Let the root viewport acquire its first render target.
		await get_tree().process_frame
		await get_tree().process_frame
		if is_inside_tree():
			var result := start_stream()
			if result != OK:
				push_error("Game Stream could not start: %s" % error_string(result))


func start_stream() -> Error:
	if Engine.is_editor_hint():
		return ERR_UNAVAILABLE
	if not Engine.has_singleton("GameStream"):
		push_error("Game Stream native library is missing. Install a platform addon package or run scripts/build.py.")
		return ERR_UNAVAILABLE
	var stream: Object = Engine.get_singleton("GameStream")
	if stream.call("is_listening"):
		return ERR_ALREADY_IN_USE
	var result: Error = stream.call(
		"listen_configured", bind_address, port, width, height, fps, codec, allow_input
	)
	_owns_stream = result == OK
	return result


func stop_stream() -> void:
	if _owns_stream and Engine.has_singleton("GameStream"):
		Engine.get_singleton("GameStream").call("stop")
	_owns_stream = false


func get_stats() -> Dictionary:
	if Engine.has_singleton("GameStream"):
		return Engine.get_singleton("GameStream").call("stats")
	return {}


func _exit_tree() -> void:
	stop_stream()
