# Audio Studio

The *Audio Studio* currently has the following tools:

* [I]nspect & Slice
* [S]tem Separation
* [D]igital Decode
* [H]f Analysis

## [I]nspect & Slice

The main view.

This supports selecting one ore more regions of audio.

### Controls

Scroll Wheel: Zoom In/Out
Mouse drag horizontally: Select region.
Shift+Scroll over selection on waveform: Change Gain (non-destructive).
Shift+Scroll over top-half of selection on spectrum: Apply Low Pass Filter.
Shift+Scroll over top-half of selection on spectrum: Apply High Pass Filter.
Ctrl+Scroll over selection: Apply crossfade of gain/filtering

Hidden from obvious controls

## [S]tem Separation

Have a song and want to isolate the vocals? Go for it.

## [D]igital Decode

This tool is centered around decoding signals that are encoded in audio.

The most basic is morse code, but it can handle DTMF (The sounds when you press buttons on a phone call) and some other, nerdier protocols too.

## [H]f Analysis

This tool is centered around listening to audio slowly.

This can mean either literally playing it back slower or shifting the pitch down.

The primary motivation for this tool is working with very high sample rate audio, like recordings you can get from the [Audio Moth](https://www.openacousticdevices.info/audiomoth). This allows for playback of bat chirps!