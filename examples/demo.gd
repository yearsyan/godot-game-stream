# SPDX-License-Identifier: LGPL-2.1-or-later
# Copyright (C) 2026 yearsyan and contributors
extends Node2D

var _elapsed: float = 0.0
var _position := Vector2(640, 360)
var _color := Color("68e5d2")
var _clicks: int = 0


func _ready() -> void:
	if "--smoke" in OS.get_cmdline_user_args():
		await get_tree().create_timer(9.0).timeout
		$StreamHost.stop_stream()
		get_tree().quit()


func _process(delta: float) -> void:
	_elapsed += delta
	_position += Input.get_vector("ui_left", "ui_right", "ui_up", "ui_down") * 320.0 * delta
	var stats: Dictionary = $StreamHost.get_stats()
	$CanvasLayer/Status.text = "Codec: %s   Encoder: %s   Remote clicks: %d" % [
		stats.get("active_codec", "waiting"), stats.get("encoder_backend", "waiting"), _clicks
	]
	queue_redraw()


func _input(event: InputEvent) -> void:
	if event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_LEFT:
		_clicks += 1
		_color = Color.from_hsv(fmod(_clicks * 0.17, 1.0), 0.55, 0.95)
		print("game_stream_demo_click=", _clicks)


func _draw() -> void:
	draw_rect(Rect2(Vector2.ZERO, get_viewport_rect().size), Color("101f30"))
	for index in range(8):
		var angle := _elapsed * 0.7 + index * TAU / 8.0
		var orbit := _position + Vector2(cos(angle), sin(angle)) * 130.0
		draw_circle(orbit, 14.0, Color(_color, 0.35))
	draw_circle(_position, 42.0 + sin(_elapsed * 2.0) * 5.0, _color)
