# Big new features:
## Image Studio
* needs a quick circle, arrow, line, and text tool
* More image FX
    * Image → particle system → image
        * Turn bright pixels into particles, let them flow under gravity/noise/vortices, then accumulate the particles back into an image.
    * Echo image — repeatedly offset and composite the image.
    * Edge feedback — edge-detect → amplify → feed back.
    * Add a new image effect called Structure to the clipboard image-processing system.
    * The goal is to reproduce the visual concept of Snapseed's "Structure" control: enhance naturally occurring texture and fine-to-medium detail throughout an image while deliberately avoiding the appearance of conventional sharpening. Object boundaries/strong edges should remain comparatively unaffected.
        * Note: Full prompt/info in ChatGPT session
* Depth-map driven contrast/saturation/brightness adjustment
## Audio Studio
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
## MIDI
* Ability to select region, copy
    * Need to test if copying pasting MIDI to bitwig works first
* I'd like a live system MIDI monitor, similar to the rolling audio meter
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