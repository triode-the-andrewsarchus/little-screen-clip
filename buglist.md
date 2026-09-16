# Bugs
## Image Studio
* A lot of the emoji icons don't render, giving us just tofu. We might need to embed a font?
    * In "🛡️ Clean Image" I see a globe and some tofu.
    * In "✕ Close (Esc)" I see a square
    * There may be others?
## Audio Studio
** The "Copy track" button is nearly falling off the screen.
* In "✕ Close (Esc)" I see a square
** The "Trim Silence" button is a different size than the other buttons near it (Multi region, snap 0 crossing, etc)
    ** This makes me worry some of the buttons aren't using our button element that correts for contrast limits
** The "All right" and "All left" buttons have poor contrast
    * The emoji in them (◨) don't render correctly either
* Pressing escape doesn't work to close the window in the Stem Separation tool
## Binary Diff
## STL viewer
* It currently doesn't auto-rotate in the little screen view
* It doesn't actually render as polygons, like you'd expect
* Some sort of false-color would be good.
## .txt preview
* .txt previews seem to add a space before the first character in the small previews
## Code previews
## PDF viewer
* There's duplicated delete and rotate buttons, one set in the top bar, and one in the big, individual page preview. I believe only the top bar ones are hooked up.
* On /home/vega/Documents/type_testing/E_US-16x08_FW-Update_vA.pdf the preview looks correct, but I still see errors trying to rotate/delete a page.
  ** Rotating works now, but the entire document turns into only white pages
** On /home/vega/Documents/type_testing/Fatal1ty X370 Gaming K4.pdf I don't see a big selected page preview, trying to delete a page makes me hang for a very long time


# Missing features
## Audio Studio
* Tools should be keyboard navigable, "[I]nspect and slice", "[S]tem Separate"
  * This works one way - going to the stem seperator, but not the other. In Stem separation, the escape button doesn't work either.
* The "[I]nspect and slice" should support deleting a slice with the delete key when hovered over that region. The waveform should recompute and "squish" to the new length
    * This works, but has no undo buffer
* Holding shift+scrolling over a selected region should change that regions amplitude up or down, but this should be non-destructive until the user:
    * opens the audio in audacity
    * copies the selected region(s)
    * This allows for silencing a region and then un-silenceing it within the same session
* Pressing [M] should toggle multi region selection when in the "[I]nspect and slice" tool
## General Workbench
## Text/code preview
* Scrolling should use a VS-Code like minimap instead of a plain scroll bar


# UI opinions
## Image Studio
## Audio Studio
## Binary Diff
* The colored background rendering vs the colored numbers can make it low contrast
* J and K are used for previous/next when arrow keys aren't bound. Bind both.
## MIDI preview
* The stop button is lower than the play button
    * This makes me worry the correct button thing we made with safe contrast isn't being used for one of them