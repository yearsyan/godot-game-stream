# SPDX-License-Identifier: LGPL-2.1-or-later
# Copyright (C) 2026 yearsyan and contributors
@tool
extends EditorPlugin


func _enter_tree() -> void:
	# Plugins can enter the tree before the first asset import has completed.
	var icon_image := Image.new()
	var icon_path: String = get_script().resource_path.get_base_dir().path_join("icon.svg")
	var result := icon_image.load_svg_from_string(FileAccess.get_file_as_string(icon_path))
	var icon: Texture2D = null
	if result == OK:
		icon = ImageTexture.create_from_image(icon_image)
	add_custom_type("GameStreamHost", "Node", preload("host.gd"), icon)


func _exit_tree() -> void:
	remove_custom_type("GameStreamHost")
