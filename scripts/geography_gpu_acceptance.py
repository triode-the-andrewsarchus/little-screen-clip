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
    return compileProgram(compileShader((root / f"{name}.vert").read_text(), GL.GL_VERTEX_SHADER),
        compileShader((root / f"{name}.frag").read_text(), GL.GL_FRAGMENT_SHADER))


def texture(internal, external, data_type, data):
    tex = GL.glGenTextures(1)
    GL.glBindTexture(GL.GL_TEXTURE_2D, tex)
    for field in [GL.GL_TEXTURE_MIN_FILTER, GL.GL_TEXTURE_MAG_FILTER]:
        GL.glTexParameteri(GL.GL_TEXTURE_2D, field, GL.GL_LINEAR)
    for field in [GL.GL_TEXTURE_WRAP_S, GL.GL_TEXTURE_WRAP_T]:
        GL.glTexParameteri(GL.GL_TEXTURE_2D, field, GL.GL_CLAMP_TO_EDGE)
    GL.glTexImage2D(GL.GL_TEXTURE_2D, 0, internal, data.shape[1], data.shape[0], 0, external, data_type, data)
    return tex


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
        assert GL.glGetError() == GL.GL_NO_ERROR
        report = {"renderer": renderer, "viewport": [width, height], "shader_programs_linked": 2,
            "raster_scenarios": len(images), "vector_scenarios": 2, "warm_frame_median_ms": statistics.median(timings),
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
