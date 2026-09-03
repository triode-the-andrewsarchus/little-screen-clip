This program is an experimental clipboard and file juggling utility, based around the idea that humans think in terms of physically handling objects and doing operations on them based on what they can actually hold: A similar skeuomorphic design to a standard videogame hot-bar.

At it's core this program needs to do 3 things

1. Display the current contents of multiple clipboard slots on a small screen - in this case a Bus 005 Device 025: ID 0416:5408 Winbond Electronics Corp. USBDISPLAY - which is a Thermalright Trofeo.
    * This means having different previews for different file types:
        * Plaintext - Truncated with an ellipsis
        * Images - A preview
        * Files - A icon with file type and size
        * Directories - A icon with directory name and size
        * Audio - The waveform, maybe with colored regions based on spectral content
        * Video - A frame from the video, or a static icon if video decoding is too expensive.
2. The ability to select one *or more* of these slots and perform actions on the data
    * This means we will need a "cursor" that can move between the slots
    * The selected item will be the one pasted by default
        * As with a normal clipboard program, if the type can't be pasted, the last copied thing that can be in history will be instead.
    * The primary actions are: 
        

Note, the "screen" does not show up to the OS as a display, it's a device we need to manager a framebuffer for manually and push that frame up to over USB.


---

Old notes on this idea:

here's my full vision: First, we grab a second small screen, probably 1920x480, that renders over USB so it's easy to connect to whatever. We won't need high refresh rate. Then, on that screen we should be able to show "slots" - think like a minecraft inventory. When you copy something, it always goes into the special "in hand" slot, but you can easily take something that's in hand and save it to one of the main inventory-hot-bar slots. On our little screen, we'd show a preview of the contents, probably inteligently. If it's an audio file, show the waveform, duration, etc. Text? Show the first N chars that will fit. Etc. Then, I should be able to run programs on the slots. Most will just act on what's in hand - they only need a single argument. But if something needs two or more, like concatenate, it will be the hand + selected slots. Whenever anything replaces what's in the hand (which most things should) it goes into a special slot that holds the pre-last-operation output, in case the user needs to undo 1 action or keep both for some reason. To make these programs that get ran, a node based system would be ideal, because it would be good to be able to make more complex flows out of simple ones, like trim white space from hand; trim white space from slot 1; concat hand + " " + s1. Scripts or python or what not can do this fine - and any node based system should have a subprocess() like system to do that, but most things should be able to be handled by the node system.

I'd like to come up with a list of 20 or so operations to start with for the first prototype - we can build the manually, without the node editor, so we can tack that part on later. There's a lot of file name operation ones that make sense, like if I copy a dozen files, allow for renaming them while I'm "holding" them in convient ways, like making them all the same name if they're all different extensions, etc. Working on strings there's a few obvious ones. URL trimming of tracking, replacing a word, concatenating with a previously copied string On audio trimming from either side, reversing, up/down sampling, filtering, raising/lowering gain On MIDI, contraining to a range, transposition on any file: getting the hash On photos: rotation, mirroring, The general rule is it should only really be operations which are easy to mentally visualize the output of, or where the change is being done for the sake of workflow (like, up/down sampling audio isn't relevant to humans usually, but the mental model isn't "Do this action" so much as "make this compatible") Think about all the things a user might put on their clipboard, actions that doing without a GUI are easy to conceptualize like this, and give me a list. Include everything I said above in that list too. Expand to any file types or clipboard contents that you think would be common.

Priror LLM input, that I've reviewed as reasonable:

Universal

These work on almost anything.

Compute SHA-256 hash
Duplicate (clone into a new object)
Compress (ZIP/TAR)
Extract archive
Convert line endings (LF ↔ CRLF) for text files
Copy as file path / URI
Copy as filename only
Copy as parent directory
Plain text

These are all immediately understandable.

Reverse words ("a b c" → "c b a")
Reverse characters
Trim whitespace
Collapse repeated whitespace
Sort lines
Remove duplicate lines
Join lines
Split into lines (using delimiter)
Find & replace
Change case (upper/lower/title)
Slugify
URL-decode
URL-encode
Remove URL tracking parameters
Base64 encode
Base64 decode
JSON pretty-print
JSON minify
Escape/unescape shell
Escape/unescape HTML
Escape/unescape Markdown
Count words/characters/lines
URLs

These are probably common enough to deserve dedicated nodes instead of treating URLs as generic strings.

Remove tracking parameters
Canonicalize URL
Extract domain
Extract path
Extract query parameters
Convert YouTube short ↔ normal URL
Remove fragment (#...)
Collections of files

I think this category is surprisingly important because file managers are often awkward.

Rename using pattern
Add prefix
Add suffix
Sequential numbering
Strip common prefix
Strip common suffix
Normalize filename spacing
Replace illegal filename characters
Change extension
Rename to hashes
Rename all to same basename while preserving extensions
Sort by date/name/size
Remove duplicates by hash
Images

Again, only things with obvious visual outcomes.

Rotate 90°
Rotate 180°
Mirror horizontally
Mirror vertically
Crop to center
Resize
Pad to square
Convert PNG/JPEG/WebP
Strip metadata
Auto-orient from EXIF
Grayscale
Invert colors

I would avoid things like "increase saturation 12%" because that's no longer trivial to visualize.

Audio

Your list is already solid.

I'd include:

Trim start
Trim end
Reverse
Normalize loudness
Gain ±XdB
Fade in
Fade out
Convert sample rate
Convert bit depth
Mono ↔ stereo
Swap channels
Low-pass filter
High-pass filter
Concatenate
Mix
Silence detection → trim silence

I'd probably avoid EQ nodes initially because the result becomes harder to predict.

MIDI

Lots of clean operations here.

Transpose
Constrain note range
Quantize
Humanize timing
Humanize velocity
Fixed velocity
Scale velocity
Remove CC events
Remove aftertouch
Change MIDI channel
Change tempo
Merge tracks
Split tracks
Remove overlapping notes
PDFs

Very workflow-oriented.

Merge
Split pages
Rotate pages
Extract text
Remove metadata
Compress
Clipboard images (screenshots)

These feel distinct from "image files."

OCR to text
Copy dominant color
Copy dimensions
Convert to PNG/JPEG
Scale to clipboard-friendly size
Archives
ZIP
TAR
Extract
List contents
Structured text

CSV, YAML, TOML, XML.

Pretty-print
Minify
Convert formats
Sort keys
Validate
Binary files

Mostly inspection.

SHA-256
MD5
Size
MIME type
Hex dump
Strings extraction
Two-input operations

These make good use of your "hand + selected slot" idea.

Text:

Concatenate
Diff
Common prefix
Common suffix

Files:

Compare hashes
Compare contents

Audio:

Concatenate
Mix
Compare duration

Images:

Side-by-side
Difference image

MIDI:

Merge
Append
A category I'd add: "clipboard utilities"

These aren't transformations, but they fit your workflow model well.

Swap hand with slot
Copy hand to slot
Append hand to collection
Duplicate selected slot
Clear slot
Restore previous result
Cycle clipboard history

---

Back to my original thoughts from that,

Something to keep in mind is on almost everything we have to "hold" the file first, in our copy buffer. Things that make us have to make a copy (since we don't want anything to be destrictive editing) of a large file will be slowwwwwwwwww, so that's also an inherit limit. I think thinking meta makes a lot more sense. there will be exceptions to the rules though, so having a lot listed out to try to put each in those categories helps.