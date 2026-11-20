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
    attrs = (EGL.EGLint * 13)(EGL.EGL_SURFACE_TYPE, EGL.EGL_PBUFFER_BIT,
        EGL.EGL_RENDERABLE_TYPE, EGL.EGL_OPENGL_BIT, EGL.EGL_RED_SIZE, 8,
        EGL.EGL_GREEN_SIZE, 8, EGL.EGL_BLUE_SIZE, 8, EGL.EGL_ALPHA_SIZE, 8, EGL.EGL_NONE)
    configs, count = (EGL.EGLConfig * 1)(), EGL.EGLint()
    assert EGL.eglChooseConfig(display, attrs, configs, 1, count) and count.value
    surface_attrs = (EGL.EGLint * 5)(EGL.EGL_WIDTH, width, EGL.EGL_HEIGHT, height, EGL.EGL_NONE)
    surface = EGL.eglCreatePbufferSurface(display, configs[0], surface_attrs)
    context_attrs = (EGL.EGLint * 7)(0x3098, 3, 0x30FB, 3, 0x30FD, 1, EGL.EGL_NONE)
    ctx = EGL.eglCreateContext(display, configs[0], EGL.EGL_NO_CONTEXT, context_attrs)
    assert surface and ctx and EGL.eglMakeCurrent(display, surface, surface, ctx)
    return display, surface, ctx


def program(root, name):
    fragment = (root / f"{name}.frag").read_text()
    if name in ("map", "sky"):
        version, body = fragment.split("\n", 1)
        fragment = f"{version}\n{(root / 'eclipse.glsl').read_text()}\n{(root / 'atmosphere.glsl').read_text()}\n{body}"
    return compileProgram(compileShader((root / f"{name}.vert").read_text(), GL.GL_VERTEX_SHADER),
        compileShader(fragment, GL.GL_FRAGMENT_SHADER))


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

    def render_sky(readable, moon_x, marker=False):
        GL.glUniform1i(uniform("u_readable_sky"), readable)
        GL.glUniform3f(uniform("u_moon"), moon_x * 384400.0, .18 * 384400.0, eye - 384400.0)
        venus = (.45 * 150e6, -.35 * 150e6, eye - 150e6) if marker else (0, 0, 0)
        GL.glUniform3f(uniform("u_planets[1]"), *venus)
        GL.glDrawArrays(GL.GL_TRIANGLES, 0, 3)
        pixels = GL.glReadPixels(0, 0, width, height, GL.GL_RGBA, GL.GL_UNSIGNED_BYTE)
        return np.frombuffer(pixels, dtype=np.uint8).reshape(height, width, 4)

    actual = render_sky(0, -.55)
    readable = render_sky(1, -.55)
    for x, threshold in [(slice(0, width // 2), 100), (slice(width // 2, width), 235)]:
        assert np.count_nonzero(readable[:, x, 0] > threshold) > 10 * np.count_nonzero(actual[:, x, 0] > threshold)
    actual_marker = render_sky(0, -.55, marker=True)[:height // 2, :, 0]
    readable_marker = render_sky(1, -.55, marker=True)[:height // 2, :, 0]
    actual_pixels = np.count_nonzero(actual_marker > 100)
    readable_pixels = np.count_nonzero(readable_marker > 100)
    assert readable_pixels >= 12 * actual_pixels > 0, (actual_pixels, readable_pixels)
    for moon_x in [.55, .525]:
        assert np.array_equal(render_sky(0, moon_x), render_sky(1, moon_x)), "Solar conjunction changed optical disks"
    GL.glDeleteTextures([background])
    GL.glDeleteProgram(sky)


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
        GL.glUniform2f(uniform("u_range"), 500, 2600)
        GL.glUniform1i(uniform("u_show_water"), 1)
        GL.glUniform1f(uniform("u_side"), 180)

        def draw(style=0, lat=0, lon=0, blend=1, zoom=1):
            GL.glUseProgram(raster)
            GL.glBindVertexArray(vao)
            GL.glClearColor(0.02, 0.03, 0.04, 1)
            GL.glClear(GL.GL_COLOR_BUFFER_BIT)
            GL.glUniform4f(uniform("u_camera"), lat, lon, min(width, height) * 0.43 * zoom, blend)
            GL.glUniform1f(uniform("u_blend"), blend)
            GL.glUniform1f(uniform("u_zoom"), zoom)
            GL.glUniform1f(uniform("u_longitude"), lon)
            GL.glUniform1i(uniform("u_style"), style)
            GL.glUniform1f(uniform("u_natural_detail"), max(0, min(1, (zoom - 5) / 4)))
            GL.glUniform1f(uniform("u_contour"), 100 if zoom >= 3.5 else 0)
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
        assert outskirts[0] > outskirts[1] > outskirts[2], outskirts
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
            Image.fromarray(np.flipud(image)).save(args.output / f"{name}.png")
        GL.glDeleteBuffers(1, [vector_buffer])
        GL.glDeleteVertexArrays(1, [vector_vao])
        # Measure warm GPU drawing with all source textures resident. Readback is
        # used only above for assertions and is excluded from these timings.
        timings = []
        for i in range(70):
            start = time.perf_counter()
            draw(1, 35, -105 + i * 0.01, 0, 6)
            GL.glFinish()
            if i >= 10:
                timings.append((time.perf_counter() - start) * 1000)
        check_sky_sizes(root, width, height)
        assert GL.glGetError() == GL.GL_NO_ERROR
        report = {"renderer": renderer, "viewport": [width, height], "shader_programs_linked": 3,
            "sky_sizing_checked": True,
            "raster_scenarios": len(images), "night_lighting_checked": True, "vector_scenarios": 2,
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
