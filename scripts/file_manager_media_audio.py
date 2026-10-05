"""Private, hardware-free PipeWire output for media acceptance."""
from pathlib import Path
import ui_acceptance_capture as private
from file_manager_acceptance import wait_for


def start_private_audio(output, env, children):
    runtime = Path(env["XDG_RUNTIME_DIR"]).resolve()
    assert runtime == (output / "runtime").resolve()
    env.update(PIPEWIRE_RUNTIME_DIR=str(runtime), PIPEWIRE_REMOTE="pipewire-0",
               PULSE_SERVER="unix:" + str(runtime / "pulse/native"))
    alsa = output / "asound.conf"
    alsa.write_text('</usr/share/alsa/alsa.conf>\npcm.!default { type pipewire playback_node "-1" }\nctl.!default { type pipewire }\n')
    env["ALSA_CONFIG_PATH"] = str(alsa)
    children.start("media-pipewire", ["pipewire"], env=env, cwd=output, log_path=output / "pipewire.log")
    wait_for(lambda: (runtime / "pipewire-0").exists(), "private PipeWire socket")
    # The policy profile excludes ALSA, Bluetooth and camera hardware monitors.
    children.start("media-policy", ["wireplumber", "-p", "policy"], env=env, cwd=output, log_path=output / "wireplumber.log")
    children.start("media-pulse", ["pipewire-pulse"], env=env, cwd=output, log_path=output / "pulse.log")
    wait_for(lambda: (runtime / "pulse/native").exists(), "private Pulse socket")
    private.run_checked(["pactl", "load-module", "module-null-sink", "sink_name=FilesAcceptance", "sink_properties=device.description=FilesAcceptance"], env=env)
    private.run_checked(["pactl", "set-default-sink", "FilesAcceptance"], env=env)
    sinks = private.run_checked(["pactl", "list", "short", "sinks"], env=env)
    assert "FilesAcceptance" in sinks
    assert len(sinks.strip().splitlines()) == 1, "acceptance must not open a hardware sink"
    return env
