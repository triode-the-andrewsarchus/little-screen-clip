#!/usr/bin/env python3
"""Compile and draw the production map shaders on a private surfaceless EGL context.

Requires PyOpenGL, NumPy, Pillow and an EGL OpenGL implementation. No compositor,
application, clipboard, display settings or downloaded map assets are touched.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import statistics
import time

os.environ.setdefault("PYOPENGL_PLATFORM", "egl")
from OpenGL import EGL, GL
from OpenGL.EGL.EXT.platform_base import eglGetPlatformDisplayEXT
from OpenGL.GL.shaders import compileProgram, compileShader
import numpy as np
from PIL import Image


def context(width, height):
    display = eglGetPlatformDisplayEXT(0x31DD, None, None)  # EGL_PLATFORM_SURFACELESS_MESA
    major, minor = EGL.EGLint(), EGL.EGLint()
    assert EGL.eglInitialize(display, major, minor)
    assert EGL.eglBindAPI(EGL.EGL_OPENGL_API)
    attrs = (EGL.EGLint * 15)(EGL.EGL_SURFACE_TYPE, EGL.EGL_PBUFFER_BIT,
        EGL.EGL_RENDERABLE_TYPE, EGL.EGL_OPENGL_BIT, EGL.EGL_RED_SIZE, 8,
        EGL.EGL_GREEN_SIZE, 8, EGL.EGL_BLUE_SIZE, 8, EGL.EGL_ALPHA_SIZE, 8,
        EGL.EGL_DEPTH_SIZE, 24, EGL.EGL_NONE)
    configs, count = (EGL.EGLConfig * 1)(), EGL.EGLint()
    assert EGL.eglChooseConfig(display, attrs, configs, 1, count) and count.value
    surface_attrs = (EGL.EGLint * 5)(EGL.EGL_WIDTH, width, EGL.EGL_HEIGHT, height, EGL.EGL_NONE)
    surface = EGL.eglCreatePbufferSurface(display, configs[0], surface_attrs)
    context_attrs = (EGL.EGLint * 7)(0x3098, 3, 0x30FB, 3, 0x30FD, 1, EGL.EGL_NONE)
    ctx = EGL.eglCreateContext(display, configs[0], EGL.EGL_NO_CONTEXT, context_attrs)
    assert surface and ctx and EGL.eglMakeCurrent(display, surface, surface, ctx)
    return display, surface, ctx


def program(root, name):
    def perspective(source):
        version, body = source.split("\n", 1)
        return f"{version}\n{(root / 'perspective.glsl').read_text()}\n{body}"

    vertex = (root / f"{name}.vert").read_text()
    if name == "map":
        vertex = perspective(vertex)
    fragment_name = "vector" if name == "terrain_vector" else name
    fragment = (root / f"{fragment_name}.frag").read_text()
    if name in ("map", "sky"):
        version, body = fragment.split("\n", 1)
        fragment = f"{version}\n{(root / 'eclipse.glsl').read_text()}\n{(root / 'atmosphere.glsl').read_text()}\n{body}"
    shaders = [
        compileShader(vertex, GL.GL_VERTEX_SHADER),
        compileShader(fragment, GL.GL_FRAGMENT_SHADER),
    ]
    if name == "terrain_vector":
        shaders.append(compileShader(
            perspective((root / "terrain_vector.geom").read_text()), GL.GL_GEOMETRY_SHADER
        ))
    return compileProgram(*shaders, validate=False)


def texture(internal, external, data_type, data):
    tex = GL.glGenTextures(1)
    GL.glBindTexture(GL.GL_TEXTURE_2D, tex)
    for field in [GL.GL_TEXTURE_MIN_FILTER, GL.GL_TEXTURE_MAG_FILTER]:
        GL.glTexParameteri(GL.GL_TEXTURE_2D, field, GL.GL_LINEAR)
    for field in [GL.GL_TEXTURE_WRAP_S, GL.GL_TEXTURE_WRAP_T]:
        GL.glTexParameteri(GL.GL_TEXTURE_2D, field, GL.GL_CLAMP_TO_EDGE)
    GL.glTexImage2D(GL.GL_TEXTURE_2D, 0, internal, data.shape[1], data.shape[0], 0, external, data_type, data)
    return tex


def check_eclipse_horizon(raster, draw, center, row, color_texture):
    # Use a uniform ocean to isolate the lighting at the first test GPX point.
    # WGS84 vectors are from DE440 at 2024-04-08 16:40 UTC; NASA puts the
    # centerline at 7.515° S, 156.4° W, with the Sun 2° above the horizon.
    uniform = lambda name: GL.glGetUniformLocation(raster, name)
    GL.glActiveTexture(GL.GL_TEXTURE0)
    ocean = texture(GL.GL_RGBA8, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE,
        np.full((2, 2, 4), [37, 104, 171, 255], np.uint8))
    GL.glUniform1f(uniform("u_earth_lighting"), 1.0)
    GL.glUniform1i(uniform("u_black_marble"), 0)
    GL.glUniform3f(uniform("u_sun"), 51799050, -139190000, 19727226)
    GL.glUniform3f(uniform("u_moon"), 118469.32, -336412.8, 46492.25)
    eclipse_rows = []
    for enabled in [0, 1]:
        GL.glUniform1i(uniform("u_solar_possible"), enabled)
        draw(lat=-7.515, lon=-158.4, blend=0, zoom=4)
        pixels = GL.glReadPixels(center - 35, row, 71, 1, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE)
        eclipse_rows.append(np.frombuffer(pixels, dtype=np.uint8).reshape(71, 4)[:, 2].astype(int))
    day, eclipsed = eclipse_rows
    dimming = day - eclipsed
    assert dimming[0] == 0 and dimming[-1] > 30
    assert np.abs(np.diff(dimming, axis=0)).max() < 12, "Hard eclipse shadow edge at the horizon"
    assert eclipsed[:-1].max() <= eclipsed[-1] + 2, "Bright band on the twilight side of totality"
    GL.glBindTexture(GL.GL_TEXTURE_2D, color_texture)
    GL.glDeleteTextures([ocean])
    GL.glUniform1i(uniform("u_solar_possible"), 0)
    GL.glUniform1f(uniform("u_earth_lighting"), 0.0)


def check_sky_sizes(root, width, height):
    sky = program(root, "sky")
    GL.glUseProgram(sky)
    uniform = lambda name: GL.glGetUniformLocation(sky, name)
    GL.glActiveTexture(GL.GL_TEXTURE0)
    background = texture(GL.GL_RGB8, GL.GL_RGB, GL.GL_UNSIGNED_BYTE, np.zeros((4, 4, 3), np.uint8))
    identity = np.eye(3, dtype=np.float32)
    for matrix in ["u_camera_basis", "u_inertial_from_fixed"]:
        GL.glUniformMatrix3fv(uniform(matrix), 1, False, identity)
    focal = .95 * height
    eye = 6378.137 * (1 + (focal / (.43 * height)) ** 2) ** .5
    GL.glUniform2f(uniform("u_fov"), width / (2 * focal), height / (2 * focal))
    GL.glUniform1f(uniform("u_camera_distance"), eye)
    GL.glUniform1f(uniform("u_opacity"), 1.0)
    GL.glUniform3f(uniform("u_sun"), .55 * 150e6, .18 * 150e6, eye - 150e6)
    GL.glUniform3f(uniform("u_lunar_sun"), 0.0, 0.0, 150e6)
    GL.glUniform3f(uniform("u_lunar_earth"), 0.0, 1e7, 0.0)

    def render_sky(readable, moon_x, marker=False, body_zoom=1.0):
        GL.glUniform1i(uniform("u_readable_sky"), readable)
        GL.glUniform1f(uniform("u_readable_disk_zoom"), body_zoom)
        GL.glUniform3f(uniform("u_moon"), moon_x * 384400.0, .18 * 384400.0, eye - 384400.0)
        venus = (.45 * 150e6, -.35 * 150e6, eye - 150e6) if marker else (0, 0, 0)
        GL.glUniform3f(uniform("u_planets[1]"), *venus)
        GL.glDrawArrays(GL.GL_TRIANGLES, 0, 3)
        pixels = GL.glReadPixels(0, 0, width, height, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE)
        return np.frombuffer(pixels, dtype=np.uint8).reshape(height, width, 4)

    actual = render_sky(0, -.55)
    readable = render_sky(1, -.55)
    for x, threshold in [(slice(0, width // 2), 100), (slice(width // 2, width), 235)]:
        assert np.count_nonzero(readable[:, x, 0] > threshold) > 100 * np.count_nonzero(actual[:, x, 0] > threshold)
    distant = render_sky(1, -.55, body_zoom=.14)
    for x, threshold in [(slice(0, width // 2), 100), (slice(width // 2, width), 235)]:
        count = lambda pixels: np.count_nonzero(pixels[:, x, 0] > threshold)
        assert count(actual) < count(distant) < count(readable) / 5
    assert np.array_equal(actual, render_sky(0, -.55, body_zoom=.14))
    actual_marker = render_sky(0, -.55, marker=True)[:height // 2, :, 0]
    readable_marker = render_sky(1, -.55, marker=True)[:height // 2, :, 0]
    actual_pixels = np.count_nonzero(actual_marker > 100)
    readable_pixels = np.count_nonzero(readable_marker > 100)
    assert readable_pixels >= 100 * actual_pixels > 0, (actual_pixels, readable_pixels)
    for moon_x in [.55, .525]:
        assert np.array_equal(render_sky(0, moon_x), render_sky(1, moon_x)), "Solar conjunction changed optical disks"

    # A dark star map with a narrow bright feature. The transient trail can add
    # a faint shoulder at the feature's edge while retaining the peak itself.
    stars = np.full((4, 4096, 3), 5, dtype=np.uint8)
    stars[:, 1990:2020] = 255
    star_texture = texture(GL.GL_RGB8, GL.GL_RGB, GL.GL_UNSIGNED_BYTE, stars)
    sharp = render_sky(0, -.55)
    GL.glUniform1f(uniform("u_star_trail"), 6 / 4096)
    trail = render_sky(0, -.55)
    lift = trail[:, :, 0].astype(int) - sharp[:, :, 0].astype(int)
    assert 3 < lift.max() < 70 and np.count_nonzero(lift) > 10
    assert np.all(lift[sharp[:, :, 0] > 100] == 0)
    GL.glDeleteTextures([star_texture])
    GL.glDeleteTextures([background])
    GL.glDeleteProgram(sky)


def check_moon_view(root, width, height):
    sky = program(root, "sky")
    GL.glUseProgram(sky)
    loc = lambda name: GL.glGetUniformLocation(sky, name)
    GL.glUniformMatrix3fv(loc("u_inertial_from_fixed"), 1, False, np.eye(3, dtype=np.float32))
    GL.glUniform2f(loc("u_fov"), width / (1.9 * height), 1 / 1.9)
    GL.glUniform3f(loc("u_moon"), -384400.0, 0.0, 0.0)
    GL.glUniform3f(loc("u_lunar_sun"), -150e6, 0.0, 0.0)
    GL.glUniform3f(loc("u_lunar_earth"), 384400.0, 0.0, 0.0)
    GL.glUniform1i(loc("u_moon_view"), 1)
    GL.glUniform1i(loc("u_moon_only"), 1)
    GL.glUniform1i(loc("u_has_lunar_albedo"), 1)
    GL.glUniform1i(loc("u_lunar_albedo"), 4)
    GL.glActiveTexture(GL.GL_TEXTURE4)
    colors = np.zeros((32, 128, 3), dtype=np.uint8)
    colors[:, :78, :] = [212, 79, 47]
    colors[:, 78:, :] = [52, 111, 211]
    albedo = texture(GL.GL_RGB8, GL.GL_RGB, GL.GL_UNSIGNED_BYTE, colors)

    def center(axes, eye, phase):
        GL.glUniformMatrix3fv(loc("u_camera_basis"), 1, False, np.asarray(axes, dtype=np.float32))
        GL.glUniform3f(loc("u_moon_eye"), *eye)
        GL.glUniform1f(loc("u_moon_phase"), phase)
        GL.glClear(GL.GL_COLOR_BUFFER_BIT)
        GL.glDrawArrays(GL.GL_TRIANGLES, 0, 3)
        return np.frombuffer(GL.glReadPixels(width // 2, height // 2, 1, 1,
            GL.GL_RGBA, GL.GL_UNSIGNED_BYTE), dtype=np.uint8)[:3]

    face = [[0, 1, 0], [0, 0, 1], [1, 0, 0]]
    side = [[-1, 0, 0], [0, 0, 1], [0, 1, 0]]
    dark = center(face, [1.7, 0, 0], 1.0)
    near = center(face, [1.02, 0, 0], 0.0)
    rotated = center(side, [0, 1.02, 0], 0.0)
    assert near[0] * .15 < dark[0] < near[0] / 3, (dark, near)
    assert near[0] > 190 and rotated[2] > 190, (near, rotated)

    # The Earth uses the cached map tiles and sits behind the separately
    # composited Moon when the three centers align on this viewing ray.
    GL.glUniform1i(loc("u_sky_eye_override"), 1)
    GL.glUniform1i(loc("u_moon_only"), 0)
    GL.glUniform1f(loc("u_opacity"), 1)
    GL.glUniform2i(loc("u_earth_tiles"), 1, 1)
    GL.glUniform1i(loc("u_earth_west"), 0)
    GL.glUniform1i(loc("u_earth_east"), 0)
    GL.glUniform1i(loc("u_stars"), 3)
    GL.glUniform3f(loc("u_sky_eye"), -387400.0, 0, 0)
    GL.glActiveTexture(GL.GL_TEXTURE0)
    earth = texture(GL.GL_RGBA8, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE,
                    np.full((2, 2, 4), [38, 180, 72, 255], dtype=np.uint8))
    GL.glActiveTexture(GL.GL_TEXTURE3)
    stars = texture(GL.GL_RGB8, GL.GL_RGB, GL.GL_UNSIGNED_BYTE, np.zeros((2, 2, 3), dtype=np.uint8))
    toward_earth = [[0, -1, 0], [0, 0, 1], [-1, 0, 0]]
    visible = center(toward_earth, [1.7, 0, 0], 0.0)
    assert visible[1] > 150, visible
    GL.glUniform1i(loc("u_moon_only"), 1)
    GL.glUniform3f(loc("u_moon_eye"), -1.7, 0, 0)
    GL.glDrawArrays(GL.GL_TRIANGLES, 0, 3)
    covered = np.frombuffer(GL.glReadPixels(width // 2, height // 2, 1, 1,
        GL.GL_RGBA, GL.GL_UNSIGNED_BYTE), dtype=np.uint8)[:3]
    assert covered[1] < 130, covered

    GL.glDeleteTextures([albedo, earth, stars])
    GL.glDeleteProgram(sky)


def check_raised_terrain(raster, vao, vertex_count, width, height, height_texture, output):
    GL.glUseProgram(raster)
    GL.glBindVertexArray(vao)
    loc = lambda name: GL.glGetUniformLocation(raster, name)
    tilt = np.deg2rad(77)
    s, c = np.sin(tilt), np.cos(tilt)
    axes = np.array([[0, 1, 0], [s, 0, c], [c, 0, -s]], dtype=np.float32)
    GL.glUniformMatrix3fv(loc("u_view_axes"), 1, False, axes)
    GL.glUniform3f(loc("u_eye"), 1 + .0025 * c, 0, -.0025 * s)
    GL.glUniform2f(loc("u_half_fov"), width / (1.9 * height), 1 / 1.9)
    GL.glUniform1f(loc("u_near"), .00004)
    GL.glUniform1i(loc("u_perspective"), 1)
    GL.glUniform3f(loc("u_patch"), 0, 0, 1)
    GL.glUniform4f(loc("u_tile"), -.35, .35, .7, 0)
    GL.glUniform4f(loc("u_camera"), 0, 0, 12000, 1)
    GL.glUniform1f(loc("u_zoom"), 100)
    GL.glUniform1f(loc("u_side"), .7)
    GL.glUniform1f(loc("u_contour"), 0)
    GL.glUniform1f(loc("u_earth_lighting"), 0)
    GL.glUniform1i(loc("u_style"), 0)
    GL.glActiveTexture(GL.GL_TEXTURE1)
    v, u = np.mgrid[0:258, 0:258]
    hill = 150.0 + 5000.0 * np.exp(-(((u - 129) / 36) ** 2 + ((v - 125) / 22) ** 2))
    mask = np.ones_like(hill)
    pixels = []
    GL.glEnable(GL.GL_DEPTH_TEST)
    for elevations in [np.full_like(hill, 150.0), hill]:
        dem = texture(GL.GL_RG32F, GL.GL_RG, GL.GL_FLOAT, np.stack([elevations, mask], axis=-1).astype(np.float32))
        GL.glClearColor(0, 0, 0, 1)
        GL.glClear(GL.GL_COLOR_BUFFER_BIT | GL.GL_DEPTH_BUFFER_BIT)
        GL.glDrawArrays(GL.GL_TRIANGLES, 0, vertex_count)
        rasterized = np.frombuffer(GL.glReadPixels(0, 0, width, height,
            GL.GL_RGBA, GL.GL_UNSIGNED_BYTE), dtype=np.uint8).reshape(height, width, 4)
        pixels.append(rasterized)
        GL.glDeleteTextures([dem])
    GL.glDisable(GL.GL_DEPTH_TEST)
    flat, raised = pixels
    silhouette = (flat[:, :, :3].max(axis=2) == 0) & (raised[:, :, :3].max(axis=2) > 50)
    Image.fromarray(np.flipud(raised)).save(output / "tilted-terrain.png")
    assert np.count_nonzero(silhouette) > 300, "Elevation should change the perspective silhouette"
    GL.glBindTexture(GL.GL_TEXTURE_2D, height_texture)
    GL.glUniform1i(loc("u_perspective"), 0)


def check_draped_vectors(root, raster, terrain_vao, vertex_count, width, height, height_texture, output):
    draped = program(root, "terrain_vector")
    side = 180.0 / 256.0
    center = side * 0.5
    lat = lon = np.deg2rad(center)
    normal = np.array([np.cos(lat) * np.cos(lon), np.cos(lat) * np.sin(lon), np.sin(lat)])
    east = np.array([-np.sin(lon), np.cos(lon), 0])
    north = np.cross(normal, east)
    tilt = np.deg2rad(77)
    s, c = np.sin(tilt), np.cos(tilt)
    axes = np.array([east, north * c + normal * s, normal * c - north * s], dtype=np.float32)
    eye = normal + axes[2] * .0025

    def apply_pose(shader):
        loc = lambda name: GL.glGetUniformLocation(shader, name)
        GL.glUseProgram(shader)
        GL.glUniformMatrix3fv(loc("u_view_axes"), 1, False, axes)
        GL.glUniform3f(loc("u_eye"), *eye)
        GL.glUniform2f(loc("u_view"), width, height)
        GL.glUniform2f(loc("u_half_fov"), width / (1.9 * height), 1 / 1.9)
        GL.glUniform1f(loc("u_near"), .00004)
        GL.glUniform1i(loc("u_perspective"), 1)
        GL.glUniform4f(loc("u_camera"), center, center, 12000, 1)
        return loc

    v, u = np.mgrid[0:258, 0:258]
    meters = 180.0 + 2600.0 * np.exp(-(((u - 128.5) / 25) ** 2 + ((v - 128.5) / 25) ** 2))
    dem_data = np.stack([meters, np.ones_like(meters)], axis=-1).astype(np.float32)
    GL.glActiveTexture(GL.GL_TEXTURE1)
    dem = texture(GL.GL_RG32F, GL.GL_RG, GL.GL_FLOAT, dem_data)
    GL.glActiveTexture(GL.GL_TEXTURE6)
    dem_atlas = GL.glGenTextures(1)
    GL.glBindTexture(GL.GL_TEXTURE_2D_ARRAY, dem_atlas)
    GL.glTexImage3D(GL.GL_TEXTURE_2D_ARRAY, 0, GL.GL_RG32F, 258, 258, 1, 0, GL.GL_RG, GL.GL_FLOAT, dem_data)
    for setting in [GL.GL_TEXTURE_MIN_FILTER, GL.GL_TEXTURE_MAG_FILTER]:
        GL.glTexParameteri(GL.GL_TEXTURE_2D_ARRAY, setting, GL.GL_LINEAR)
    GL.glActiveTexture(GL.GL_TEXTURE7)
    key = ((8 << 24) | (127 << 12) | 256) + 1
    pages = np.zeros((1, 512, 4), dtype=np.uint32)
    pages[0, ((key * 2654435761) & 0xffffffff) >> 23] = [key, 1, 8, 32]
    page_table = texture(GL.GL_RGBA32UI, GL.GL_RGBA_INTEGER, GL.GL_UNSIGNED_INT, pages)
    for setting in [GL.GL_TEXTURE_MIN_FILTER, GL.GL_TEXTURE_MAG_FILTER]:
        GL.glTexParameteri(GL.GL_TEXTURE_2D, setting, GL.GL_NEAREST)

    loc = apply_pose(raster)
    GL.glUniform3f(loc("u_patch"), 0, 0, 1)
    GL.glUniform4f(loc("u_tile"), -center, side, side, 0)
    GL.glUniform1f(loc("u_side"), side)
    GL.glClear(GL.GL_COLOR_BUFFER_BIT | GL.GL_DEPTH_BUFFER_BIT)
    GL.glEnable(GL.GL_DEPTH_TEST)
    GL.glBindVertexArray(terrain_vao)
    GL.glDrawArrays(GL.GL_TRIANGLES, 0, vertex_count)
    GL.glDepthMask(GL.GL_FALSE)

    vao, buffer = GL.glGenVertexArrays(1), GL.glGenBuffers(1)
    GL.glBindVertexArray(vao)
    GL.glBindBuffer(GL.GL_ARRAY_BUFFER, buffer)
    for index, size, offset in [(0, 4, 0), (1, 2, 16)]:
        GL.glEnableVertexAttribArray(index)
        GL.glVertexAttribPointer(index, size, GL.GL_FLOAT, False, 24, ctypes.c_void_p(offset))
        GL.glVertexAttribDivisor(index, 1)

    loc = apply_pose(draped)
    GL.glUniform1i(loc("u_terrain_dem"), 6)
    GL.glUniform1i(loc("u_terrain_pages"), 7)
    GL.glUniform1i(loc("u_dem_level"), 8)
    GL.glUniform1f(loc("u_drape_step"), side / 32.0)
    GL.glUniform1f(loc("u_width"), 3)
    GL.glUniform4f(loc("u_color"), 1, 0, 0, 1)

    def line(record):
        data = np.array(record, dtype=np.float32)
        GL.glBufferData(GL.GL_ARRAY_BUFFER, data.nbytes, data, GL.GL_STATIC_DRAW)
        GL.glDrawArraysInstanced(GL.GL_POINTS, 0, 4, 1)
        return np.frombuffer(GL.glReadPixels(0, 0, width, height, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE),
            dtype=np.uint8).reshape(height, width, 4)

    hidden = line([center - .018, .54, center + .018, .54, 0, 0])
    assert not np.any(hidden[:, :, 0] > 240), "The ridge must hide a line on its far side"
    image = line([.10, center, side - .10, center, 0, 0])
    crest = normal * (1 + 2780.0 / 6371000.0)
    offset = axes @ (crest - eye)
    crest_y = height / 2 + height * .95 * offset[1] / -offset[2]
    Image.fromarray(np.flipud(image)).save(output / "draped-vector.png")
    assert np.any(image[int(crest_y)-3:int(crest_y)+4, width//2-8:width//2+9, 0] > 240), (
        "The line must rise with the mesh between its low endpoints", crest_y
    )

    assert np.count_nonzero((image[:, 20:-20, 0] > 190).any(axis=0)) > width - 46, (
        "The draped line must stay continuous across the rise and descent"
    )

    GL.glDepthMask(GL.GL_TRUE)
    GL.glDisable(GL.GL_DEPTH_TEST)
    GL.glDeleteBuffers(1, [buffer])
    GL.glDeleteVertexArrays(1, [vao])
    GL.glDeleteTextures([dem, dem_atlas, page_table])
    GL.glDeleteProgram(draped)
    GL.glActiveTexture(GL.GL_TEXTURE1)
    GL.glBindTexture(GL.GL_TEXTURE_2D, height_texture)
    GL.glUseProgram(raster)
    GL.glUniform1i(GL.glGetUniformLocation(raster, "u_perspective"), 0)


def check_vectors(vectors, width, height, output):
    vector_vao, vector_buffer = GL.glGenVertexArrays(1), GL.glGenBuffers(1)
    GL.glBindVertexArray(vector_vao)
    GL.glBindBuffer(GL.GL_ARRAY_BUFFER, vector_buffer)
    for index, size, offset in [(0, 4, 0), (1, 2, 16)]:
        GL.glEnableVertexAttribArray(index)
        GL.glVertexAttribPointer(index, size, GL.GL_FLOAT, False, 24, ctypes.c_void_p(offset))
        GL.glVertexAttribDivisor(index, 1)
    GL.glUseProgram(vectors)
    vu = lambda name: GL.glGetUniformLocation(vectors, name)
    GL.glUniform4f(vu("u_camera"), 0, 0, 300, 0)
    GL.glUniform2f(vu("u_view"), width, height)
    GL.glUniform1f(vu("u_blend"), 0)
    GL.glUniform4f(vu("u_color"), 1, 0, 0, 1)
    GL.glUniform1i(vu("u_dashed"), 0)
    GL.glUniform1f(vu("u_shift"), 0)
    for name, instance, stroke, count in [
        ("vector-line", [-5, 0, 5, 0, 1, 0], 4, 6),
        ("vector-triangle", [-5, -5, 5, -5, 0, 5], 0, 3)]:
        data = np.array(instance, dtype=np.float32)
        GL.glBufferData(GL.GL_ARRAY_BUFFER, data.nbytes, data, GL.GL_STATIC_DRAW)
        GL.glUniform1f(vu("u_width"), stroke)
        GL.glClear(GL.GL_COLOR_BUFFER_BIT)
        GL.glDrawArraysInstanced(GL.GL_TRIANGLES, 0, count, 1)
        pixels = GL.glReadPixels(0, 0, width, height, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE)
        image = np.frombuffer(pixels, dtype=np.uint8).reshape(height, width, 4)
        red = (image[:, :, 0] > 240) & (image[:, :, 1] < 20)
        assert np.count_nonzero(red) > 100, f"Vector instance failed: {name}"
        Image.fromarray(np.flipud(image)).save(output / f"{name}.png")
        if name == "vector-line":
            GL.glUniform1f(vu("u_earth_lighting"), 1)
            for dimming in [.60, .78]:  # Country boundaries and coastlines.
                GL.glUniform1f(vu("u_night_dimming"), dimming)
                for sun_x, expected in [(-1.0, 1.0 - dimming), (1.0, 1.0)]:
                    GL.glUniform3f(vu("u_sun"), sun_x, 0, 0)
                    GL.glClear(GL.GL_COLOR_BUFFER_BIT)
                    GL.glDrawArraysInstanced(GL.GL_TRIANGLES, 0, count, 1)
                    border = np.frombuffer(GL.glReadPixels(0, 0, width, height, GL.GL_RGBA,
                        GL.GL_UNSIGNED_BYTE), dtype=np.uint8).reshape(height, width, 4)
                    assert abs(int(border[:, :, 0].max()) - expected * int(image[:, :, 0].max())) <= 1
            GL.glUniform1f(vu("u_night_dimming"), 0)
    GL.glDeleteBuffers(1, [vector_buffer])
    GL.glDeleteVertexArrays(1, [vector_vao])


def check_constellation_projection(root, width, height):
    figures = program(root, "figures")
    vao, vbo = GL.glGenVertexArrays(1), GL.glGenBuffers(1)
    GL.glBindVertexArray(vao)
    GL.glBindBuffer(GL.GL_ARRAY_BUFFER, vbo)
    directions = np.array([[-.28, -.16, -1], [.35, .21, -1],
        [-.28, .16, 1], [.35, -.21, 1]], dtype=np.float32)
    GL.glBufferData(GL.GL_ARRAY_BUFFER, directions.nbytes, directions, GL.GL_STATIC_DRAW)
    GL.glEnableVertexAttribArray(0)
    GL.glVertexAttribPointer(0, 3, GL.GL_FLOAT, False, 12, ctypes.c_void_p(0))
    GL.glUseProgram(figures)
    uniform = lambda name: GL.glGetUniformLocation(figures, name)
    focal = .95 * min(width, height)
    GL.glUniform2f(uniform("u_fov"), width / (2 * focal), height / (2 * focal))
    GL.glUniform1f(uniform("u_opacity"), 1)
    GL.glClearColor(0, 0, 0, 1)
    for theta in [0, .37]:
        s, c = np.sin(theta), np.cos(theta)
        axes = np.array([[c, s, 0], [-s, c, 0], [0, 0, 1]], dtype=np.float32)
        GL.glUniformMatrix3fv(uniform("u_camera_inertial"), 1, False, axes)
        GL.glClear(GL.GL_COLOR_BUFFER_BIT)
        GL.glDrawArrays(GL.GL_LINES, 0, len(directions))
        pixels = np.frombuffer(GL.glReadPixels(0, 0, width, height, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE),
            dtype=np.uint8).reshape(height, width, 4)
        camera = directions[:2] @ axes.T
        x, y = (camera[:, :2] * focal + [width / 2, height / 2]).T
        for px, py in zip(x, y):
            assert pixels[round(py)-2:round(py)+3, round(px)-2:round(px)+3, 2].max() > 100
        drawn_y, drawn_x = np.where(pixels[:, :, 2] > 0)
        deviation = abs((drawn_x + .5 - x[0]) * (y[1] - y[0]) - (drawn_y + .5 - y[0]) * (x[1] - x[0]))
        assert len(drawn_x) > 100 and deviation.max() < np.hypot(x[1] - x[0], y[1] - y[0])
    GL.glDeleteBuffers(1, [vbo])
    GL.glDeleteVertexArrays(1, [vao])
    GL.glDeleteProgram(figures)


def check_natural_terrain_tone(raster, draw, width, height, textures):
    uniform = lambda name: GL.glGetUniformLocation(raster, name)
    GL.glActiveTexture(GL.GL_TEXTURE0)
    color = np.full((258, 258, 4), [181, 160, 124, 255], np.uint8)
    color[:129, :, :3] = [119, 152, 105]
    base = texture(GL.GL_RGBA8, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE, color)
    GL.glActiveTexture(GL.GL_TEXTURE1)
    dem = np.empty((258, 258, 2), dtype=np.float32)
    dem[:] = [320, 0]
    heights = texture(GL.GL_RG32F, GL.GL_RG, GL.GL_FLOAT, dem)
    GL.glUniform1f(uniform("u_side"), .25)

    def frame(zoom, interval=0, rgb=False, scale_zoom=None):
        draw(lon=45, blend=0, zoom=zoom, interval=interval, scale_zoom=scale_zoom)
        pixels = GL.glReadPixels(0, 0, width, height, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE)
        color_pixels = np.frombuffer(pixels, np.uint8).reshape(height, width, 4)[:, :, :3]
        return color_pixels if rgb else color_pixels @ [.299, .587, .114]

    no_dem = frame(12.0)
    for height_m in [320.0, 610.0]:
        # Simulate arrival of a different terrain level at a static camera.
        dem[:] = [height_m, 1.0]
        GL.glTexSubImage2D(GL.GL_TEXTURE_2D, 0, 0, 0, 258, 258, GL.GL_RG, GL.GL_FLOAT, dem)
        for zoom in [4.0, 6.0, 7.5, 9.0, 14.0, 28.0]:
            measured = frame(zoom)
            for row in [height // 4, 3 * height // 4]:
                assert abs(measured[row, width // 2] - no_dem[row, width // 2]) <= 1, (height_m, zoom, row)

    # A steep ridge with enough sub-pixel contours to look gray if every line
    # is drawn at full opacity. Index contours must still be visible.
    dem[:, :, 0] = 8000 + 1300 * np.abs(np.arange(258) - 64)
    GL.glTexSubImage2D(GL.GL_TEXTURE_2D, 0, 0, 0, 258, 258, GL.GL_RG, GL.GL_FLOAT, dem)
    relief = frame(12.0)
    assert np.mean(np.abs(relief - no_dem)) > 10.0, "Terrain relief disappeared"
    coarse = frame(12.0, 100)
    fine = frame(12.0, 50)
    full_field_dim = np.mean(relief - fine)
    step_dim = np.mean(coarse - fine)
    assert full_field_dim < 8.0 and abs(step_dim) < 3.0, (full_field_dim, step_dim)
    assert np.max(relief - fine) > 10.0, "Index contours disappeared"

    # Drive a gradual change from green through yellow-green into brown at
    # constant elevation. The derived palette should add intermediate colors.
    dem[:] = [320, 1]
    GL.glTexSubImage2D(GL.GL_TEXTURE_2D, 0, 0, 0, 258, 258, GL.GL_RG, GL.GL_FLOAT, dem)
    color[:, :, :3] = [143, 143, 106]
    color[:, :, 0] = np.clip(143 + 3 * (np.arange(258) - 64), 30, 240)
    GL.glActiveTexture(GL.GL_TEXTURE0)
    GL.glTexSubImage2D(GL.GL_TEXTURE_2D, 0, 0, 0, 258, 258, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE, color)
    row = frame(12.0, rgb=True)[height // 2].astype(int)
    assert len(np.unique(row, axis=0)) > 30
    assert np.abs(np.diff(row, axis=0)).max() <= 2, "Hard palette boundary in a continuous source"

    # Keep a common footprint to isolate the style from the change in camera scale.
    color[:, :, :3] = [173, 145, 109]
    color[:129, :, :3] = [118, 149, 99]
    tint = np.clip((np.arange(258) - 64) * 4, -38, 38)
    color[:, :, :3] = color[:, :, :3] + tint[None, :, None]
    GL.glTexSubImage2D(GL.GL_TEXTURE_2D, 0, 0, 0, 258, 258, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE, color)
    mid, close = [frame(zoom, rgb=True, scale_zoom=16.0).astype(int) for zoom in (28.0, 128.0)]
    for row in (height // 4, height * 3 // 4):
        mid_span = np.ptp(mid[row] @ [.299, .587, .114])
        close_span = np.ptp(close[row] @ [.299, .587, .114])
        assert mid_span > 15 and close_span < .65 * mid_span, (mid_span, close_span)
    assert close[height * 3 // 4, width // 2, 1] > close[height * 3 // 4, width // 2, 0]
    assert close[height // 4, width // 2, 0] > close[height // 4, width // 2, 1]

    GL.glActiveTexture(GL.GL_TEXTURE1)
    dem[:, :, 0] = 340 + 18 * np.abs(np.arange(258) - 64)
    GL.glTexSubImage2D(GL.GL_TEXTURE_2D, 0, 0, 0, 258, 258, GL.GL_RG, GL.GL_FLOAT, dem)
    mid_relief = frame(28.0, scale_zoom=16.0) - (mid @ [.299, .587, .114])
    close_relief = frame(128.0, scale_zoom=16.0) - (close @ [.299, .587, .114])
    assert np.mean(np.abs(close_relief)) > 1.45 * np.mean(np.abs(mid_relief))

    for unit in (0, 1):
        GL.glActiveTexture(GL.GL_TEXTURE0 + unit)
        GL.glBindTexture(GL.GL_TEXTURE_2D, textures[unit])
    GL.glDeleteTextures([base, heights])
    GL.glUniform1f(uniform("u_side"), 180)


def warm_frame_timings(draw):
    # Measure warm GPU drawing with all source textures resident. Readback is
    # used only above for assertions and is excluded from these timings.
    timings = []
    for i in range(70):
        start = time.perf_counter()
        draw(1, 35, -105 + i * 0.01, 0, 6)
        GL.glFinish()
        if i >= 10:
            timings.append((time.perf_counter() - start) * 1000)
    return timings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("/tmp/geography-gpu-acceptance"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    root = Path(__file__).resolve().parents[1] / "crates/clip-file-manager/src/file_manager/geography"
    width, height = 512, 384
    display, surface, ctx = context(width, height)
    try:
        raster, vectors = program(root, "map"), program(root, "vector")
        renderer = GL.glGetString(GL.GL_RENDERER).decode()
        grid = np.array([((x + dx) / 32, (y + dy) / 32) for y in range(32) for x in range(32)
            for dx, dy in [(0, 0), (1, 0), (0, 1), (0, 1), (1, 0), (1, 1)]], dtype=np.float32)
        vao, buffer = GL.glGenVertexArrays(1), GL.glGenBuffers(1)
        GL.glBindVertexArray(vao)
        GL.glBindBuffer(GL.GL_ARRAY_BUFFER, buffer)
        GL.glBufferData(GL.GL_ARRAY_BUFFER, grid.nbytes, grid, GL.GL_STATIC_DRAW)
        GL.glEnableVertexAttribArray(0)
        GL.glVertexAttribPointer(0, 2, GL.GL_FLOAT, False, 8, ctypes.c_void_p(0))
        y, x = np.mgrid[0:258, 0:258]
        color = np.stack([70 + x // 3, 90 + y // 3, np.full_like(x, 80), np.full_like(x, 255)], axis=-1).astype(np.uint8)
        elevation = np.stack([500 + x * 5 + y * 3, np.ones_like(x)], axis=-1).astype(np.float32)
        water = np.zeros((258, 258), dtype=np.uint8)
        textures = [texture(GL.GL_RGBA8, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE, color),
            texture(GL.GL_RG32F, GL.GL_RG, GL.GL_FLOAT, elevation),
            texture(GL.GL_R8, GL.GL_RED, GL.GL_UNSIGNED_BYTE, water)]
        GL.glViewport(0, 0, width, height)
        GL.glUseProgram(raster)
        uniform = lambda name: GL.glGetUniformLocation(raster, name)
        for unit, name in enumerate(["u_color", "u_height", "u_water"]):
            GL.glActiveTexture(GL.GL_TEXTURE0 + unit)
            GL.glBindTexture(GL.GL_TEXTURE_2D, textures[unit])
            GL.glUniform1i(uniform(name), unit)
        GL.glUniform2f(uniform("u_view"), width, height)
        GL.glUniform1i(uniform("u_show_water"), 1)
        GL.glUniform1f(uniform("u_side"), 180)

        def draw(style=0, lat=0, lon=0, blend=1, zoom=1, interval=None, scale_zoom=None):
            GL.glUseProgram(raster)
            GL.glBindVertexArray(vao)
            GL.glClearColor(0.02, 0.03, 0.04, 1)
            GL.glClear(GL.GL_COLOR_BUFFER_BIT)
            GL.glUniform4f(uniform("u_camera"), lat, lon, min(width, height) * 0.43 * (zoom if scale_zoom is None else scale_zoom), blend)
            GL.glUniform1f(uniform("u_blend"), blend)
            GL.glUniform1f(uniform("u_zoom"), zoom)
            GL.glUniform1f(uniform("u_longitude"), lon)
            GL.glUniform1i(uniform("u_style"), style)
            GL.glUniform1f(uniform("u_contour"), (100 if zoom >= 3.5 else 0) if interval is None else interval)
            for west in [-180, 0]:
                relative = (west + 90 - lon + 180) % 360 - 180 - 90
                for shift in [-360, 0, 360]:
                    if relative + shift > 180 or relative + shift + 180 < -180:
                        continue
                    GL.glUniform4f(uniform("u_tile"), relative + shift, 90, 180, 0)
                    GL.glDrawArrays(GL.GL_TRIANGLES, 0, len(grid))

        images = []
        for name, style, lat, lon, blend, zoom in [
            ("natural-globe", 0, 0, 0, 1, 1), ("panned-globe", 0, 35, -105, 1, 1),
            ("transition", 0, 35, -105, 0.5, 3.15), ("flat-contours", 1, 35, -105, 0, 6),
            ("natural-detail", 0, 35, -105, 0, 12),
            ("oilslick", 2, 35, -105, 0, 6), ("hillshade", 3, 35, -105, 0, 6)]:
            draw(style, lat, lon, blend, zoom)
            pixels = GL.glReadPixels(0, 0, width, height, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE)
            image = Image.frombytes("RGBA", (width, height), pixels).transpose(Image.Transpose.FLIP_TOP_BOTTOM)
            image.save(args.output / f"{name}.png")
            assert len(np.unique(np.asarray(image.convert("RGB")).reshape(-1, 3), axis=0)) > 10, f"Empty or constant map: {name}"
            images.append(pixels)
        assert len(set(images)) == len(images), "Camera/style changes did not affect rendering"
        check_natural_terrain_tone(raster, draw, width, height, textures)

        # Flat water at zero elevation is common in coastal DEMs. Use a
        # diagonal bank to check that the transitions interpolate off-grid.
        coastal = color.copy()
        coastal[:, :, :3] = [120, 160, 95]
        slope = x + y > 258
        shoreline = np.stack([350.0 * slope, np.ones_like(x)], axis=-1).astype(np.float32)
        river_bank = np.where(slope, 128, 255).astype(np.uint8)
        gl_inputs = [(GL.GL_RGBA, GL.GL_UNSIGNED_BYTE), (GL.GL_RG, GL.GL_FLOAT), (GL.GL_RED, GL.GL_UNSIGNED_BYTE)]
        for label, height_data, mask_data in [
            ("coastal-edge", shoreline, water), ("river-mask-edge", elevation, river_bank)
        ]:
            for unit, data in enumerate([coastal, height_data, mask_data]):
                GL.glActiveTexture(GL.GL_TEXTURE0 + unit)
                GL.glTexSubImage2D(GL.GL_TEXTURE_2D, 0, 0, 0, 258, 258, *gl_inputs[unit], data)
            draw(lat=0, lon=90, blend=0, zoom=30)
            pixels = GL.glReadPixels(0, 0, width, height, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE)
            image = np.frombuffer(pixels, dtype=np.uint8).reshape(height, width, 4)
            assert np.count_nonzero((image[:, :, 1] > image[:, :, 2]) & (image[:, :, 2] > 85)) > 100
            blended = (image[:, :, 1] > 85) & (abs(image[:, :, 1].astype(int) - image[:, :, 2]) < 12)
            assert np.count_nonzero(blended) > 30, label
            Image.fromarray(np.flipud(image)).save(args.output / f"{label}.png")
        for unit, data in enumerate([color, elevation, water]):
            GL.glActiveTexture(GL.GL_TEXTURE0 + unit)
            GL.glTexSubImage2D(GL.GL_TEXTURE_2D, 0, 0, 0, 258, 258, *gl_inputs[unit], data)

        # A white center, a dimmer suburb, and the JPEG's blue background.
        GL.glActiveTexture(GL.GL_TEXTURE5)
        longitude = np.abs((np.arange(720) + 0.5) / 2 - 180)
        night_source = np.tile([49, 49, 79], (360, 720, 1)).astype(np.uint8)
        night_source[:, longitude < 2.5] = [102, 102, 102]
        night_source[:, longitude < 1] = [255, 255, 255]
        night_texture = texture(GL.GL_RGB8, GL.GL_RGB, GL.GL_UNSIGNED_BYTE, night_source)
        GL.glTexParameteri(GL.GL_TEXTURE_2D, GL.GL_TEXTURE_MIN_FILTER, GL.GL_LINEAR_MIPMAP_LINEAR)
        GL.glGenerateMipmap(GL.GL_TEXTURE_2D)
        GL.glUniform1i(uniform("u_night"), 5)

        def lit_exposure(sun_x, lights, blend=0, zoom=3, exposure=1.0):
            GL.glUniform3f(uniform("u_sun"), sun_x, 0, 0)
            GL.glUniform1i(uniform("u_black_marble"), lights)
            GL.glUniform1f(uniform("u_earth_lighting"), exposure)
            draw(blend=blend, zoom=zoom)
            return np.frombuffer(GL.glReadPixels(0, height // 2, width, 1, GL.GL_RGBA,
                GL.GL_UNSIGNED_BYTE), dtype=np.uint8).reshape(width, 4)[:, :3].astype(int)

        unlit = lit_exposure(-150e6, 0)
        lit = lit_exposure(-150e6, 1)
        center = width // 2
        core, outskirts = lit[center], lit[center + 16]
        assert min(core) > 220 and max(core) - min(core) < 32, core
        warm_shift = outskirts - unlit[center + 16]
        assert warm_shift[0] > warm_shift[1] > warm_shift[2], warm_shift
        assert 12 < outskirts[0] - unlit[center + 16, 0] < 95
        assert 0 < lit[center + 23, 0] - unlit[center + 23, 0] < 12
        assert np.array_equal(lit[center + 40], unlit[center + 40])
        assert np.array_equal(lit_exposure(150e6, 0), lit_exposure(150e6, 1))
        day_palette = lit_exposure(-150e6, 1, exposure=0.0)
        assert np.array_equal(day_palette, lit_exposure(150e6, 1, exposure=0.0))
        half = lit_exposure(-150e6, 1, exposure=0.5)
        assert np.all(np.abs(half - (lit + day_palette) / 2) <= 1)
        assert np.all(lit_exposure(-150e6, 0, 1, 1)[center] < unlit[center])
        assert np.array_equal(lit_exposure(-150e6, 0, 1, 4)[center],
            lit_exposure(-150e6, 0, 0, 4)[center])
        faded = lit_exposure(-150e6, 1, zoom=6) - lit_exposure(-150e6, 0, zoom=6)
        assert 0 < faded[center, 0] < .5 * (lit[center, 0] - unlit[center, 0])
        assert np.array_equal(lit_exposure(-150e6, 1, zoom=10), lit_exposure(-150e6, 0, zoom=10))

        check_eclipse_horizon(raster, draw, center, height // 2, textures[0])
        GL.glDeleteTextures([night_texture])

        check_raised_terrain(raster, vao, len(grid), width, height, textures[1], args.output)
        check_draped_vectors(root, raster, vao, len(grid), width, height, textures[1], args.output)
        check_vectors(vectors, width, height, args.output)
        timings = warm_frame_timings(draw)
        check_sky_sizes(root, width, height)
        check_moon_view(root, width, height)
        check_constellation_projection(root, width, height)
        assert GL.glGetError() == GL.GL_NO_ERROR
        report = {"renderer": renderer, "viewport": [width, height], "shader_programs_linked": 5,
            "sky_sizing_checked": True, "natural_terrain_tone_checked": True,
            "raster_scenarios": len(images), "night_lighting_checked": True, "vector_scenarios": 2,
            "raised_terrain_checked": True, "draped_vectors_checked": True, "moon_view_checked": True,
            "warm_frame_median_ms": statistics.median(timings),
            "warm_frame_p95_ms": sorted(timings)[int(len(timings) * 0.95)]}
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
        GL.glDeleteTextures(textures)
        GL.glDeleteBuffers(1, [buffer])
        GL.glDeleteVertexArrays(1, [vao])
        GL.glDeleteProgram(raster)
        GL.glDeleteProgram(vectors)
    finally:
        EGL.eglMakeCurrent(display, EGL.EGL_NO_SURFACE, EGL.EGL_NO_SURFACE, EGL.EGL_NO_CONTEXT)
        EGL.eglDestroyContext(display, ctx)
        EGL.eglDestroySurface(display, surface)
        EGL.eglTerminate(display)


if __name__ == "__main__":
    main()
