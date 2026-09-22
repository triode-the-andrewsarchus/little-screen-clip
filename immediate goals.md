# Overall
- Use https://github.com/Rust-GPU/rust-gpu to get rendering times down
- Make sure as much as possible is keyboard driven, like numbers to get to the tools in the image markup editor

# Big new features:
## Image Studio
* More image FX
    * Image → particle system → image
        * Turn bright pixels into particles, let them flow under gravity/noise/vortices, then accumulate the particles back into an image.
    * Echo image — repeatedly offset and composite the image.
    * Edge feedback — edge-detect → amplify → feed back.
    * https://github.com/Xpktro/bndr port
* Depth-map driven contrast/saturation/brightness adjustment
## Audio Studio
* Resample / change sample rate
* Channel operations — stereo→mono, extract L/R, swap L/R, invert polarity, perhaps copy L→R/R→L.
* '+' over sample to get next region of the same size, that way a lot of drums can be extracted quickly.
    * This requires some careful thinking about how to handle 0-crossing and user intent vs the obvious
    * We might need to store the original target transient point targets seprately from the actual selecting boundry
* Reverse support
* Speech Fix
    * DeepFilterNet3 — primary speech denoiser.
    * ClearerVoice/MossFormer2_SE_48K — heavyweight speech rescue.
    * Um remover?
* Repair
    * Cathar — general-purpose restoration workhorse: de-click, de-crackle, de-clip, de-plosive, de-hum, de-wind, etc.
    * audiojs/denoise — useful lightweight/browser-compatible implementations and a good source of conventional algorithms.
* De-Verb
    * BUDDy — experimental/high-quality speech dereverb.
    * U-DREAM — newer experimental dereverb candidate.
* Some sort of indication of stereo separation on stereo files
* Copy spectrum as image, copy waveform as image options
## MIDI
* I'd like a live system MIDI monitor, similar to the rolling audio meter
## PDF
* add epub support
* convert epub <-> pdf
## 3D file viewer
* More file support
## .ply/.splat preview
* The thing
## Video
* Basic preview
* grab selection to clip .mkv/.mp4 or .gif
## Diff (both binary and text)
- Show SHA1 of each file at top
## Color picker
- Lazy multi-color space color picker
- Should also be able to pick from screen (?)
## Node machine
- Allow machine to be used for auto actions
- More nodes. So many more nodes.


# future goals:

- replace app launcher with per-task system with stupid fast navigation
    - could type app name, but the point here is you can type what you want to do instead, and then get a easy, normal, and power-user tool list
- Unfortunately, we probably need to re-invet the wheel and eat the file manager for this too
- Meta, we might be able to make MCP/AI stuff for integration
- Impliment some of the https://mahlet-inc.github.io/#displays visualizers

# very-future goals:

- Replace file manager
    - Filemanager: Conflict between computer using the FS and the user. Lean into that, make user pinning and organizing on top of it a thing.
- The calculator to end all calculators
    - Always saved history
    - Unit conversion
    - Computer algebra system
- hardware controller integration - MIDI control?