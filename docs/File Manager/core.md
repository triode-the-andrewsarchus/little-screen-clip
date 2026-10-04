# File Manager

It does all the basic things you'd expect. I'm going to assume you understand the basics of going though folders, copy paste, etc. in any normal file manager.

## Notable Features

### Modify Filesystem as a File

VS Code extension-alike

### The Shelf and Clipboard System

The Shelf, is an extension of the hand/inventory system. 

#### Make Files from Clipboard Contents

If you copy some text, you should be able to paste directly and have it make a text file. If you copy a picture from the internet, ctrl+v should put that picture into a folder. It should just work.

### Previews ... like, All of Them.

A normal file manager has support for image, PDF, and a few more basics. Sometimes, on some OS's, 3rd party programs can add handlers for other types. But, generally, previews/thumbnails are VERY limited.

Obviously, no one program can handle everything, but Little Screen Clip goes a lot further. You'll get actual waveform previews for audio, page counts on PDFs, etc.

### Search

#### Normal Name-Match Searching

Just type. (Similar to Thunar, or sorta-kinda MS explorer.exe)

By default, it only searches the current folder + any expanded preview folders (folders with N items or less that auto-expanded). If there's no matches, it'll start a recursive search from the current directory, plus any locations set to always search.

Also searches for subsequences

a_foo_b.md should be able to be found by typing "a_" "foo" 

Assumes any character between name and dot if no perfect math, so foo.md will still find foo_2.md

Still no match? Starts fuzzy searching with the input, with some typo tolerance

#### (Meta)Data Searching

* rg-a style search in documents
* greater than / less than size in B/KB/MB/GB/TB, between with 10kb...2MB
* glob searching
  * *.png
  * Image, Video, Auido, 3D, Document short hands for globs of those types of files
  * 

### Rules

Node graph

Want any .zips in a folder to auto-extract and delete the archive? Easy.
Want any .xls files to auto-convert to .odt. Done.
Want any images in a given folder to be converted to .webp, resized to be no more than 2000px on the biggest size, and have the metadata edited? Add more nodes.
Want to highlight any folder which has a parent .zip in the same folder? Want to make it a different color if the contents have changed?
Want to have anything that would be "foo (1)" to "foo (mm-dd-yyyy)"? Just another rule.

### A Real Home

No, not `~`, a real Home. Pinned folders and files, nothing you don't want. Smart assumptions (that you can modify! Just nodes!) about what to show contextually. Want the last N downloads that are from less than 2 hours ago? Just a rule. Recent files from Syncthing, to get changed files from your phone or laptop? Done.

### A Real Sorting System

Real folders are often messy. Sorting based on file attributes (like the artist/genre in a folder of music) shouldn't fail just because there's a few pictures or a text document in there too. A configurable threshold sets what percent of files in a folder need to be the same type (not file extension! You care that they're audio, not .wav vs .mp3). This can always be overriden in a folder (or parent folder, inherited by the folder tree) to force it to be a view for a that type.

Additionally, you can actually sort things by meaningful human things, not just file size or computer-y things. Warm/Cool for pictures. etc.

### Tree View

https://en.wikipedia.org/wiki/Miller_columns w/ file preview

### Smart Expansion

In a typical file manager, if you're in a folder with a dozen folders in it, each with only a handful of files in them, you're stuck either:

1. Going in them one-by one to see their contents
2. Using a weirdly dated looking tree view.

Both are still obviously options, but you can also just have the folder pre-expand inline, showing whats in it if it contains less than a configurable (and overrideable!) number of items, with a colored background making it clear it's from this expansion.

### Session Mode

Sometimes you want to mark that a folder or file is "done" for just some in-progress task. Maybe your sorting pictures and have a few folders that are already sorted. Just mark them for this session as already handled.

What states mean is up to you - so "done" isn't what this is actually called, it's just Red, Yellow, and Green. The assumption is these will probably be "Done", "In progress", "Not Started" in most uses, but meaning is up to you, the user. These render as solid lines under the folder or file.

### A/B/... Sorting

Sometimes you need to bulk sort a bunch of files. Imagine a hundred pictures, some of animals, some of guitars. You could mark them as Red and Blue. But, wait, crap, now you want to sub-sort the animals into dogs/cats and other! You can select to show just the Red items, and re-color them Red or Orange. Then you can do the same for the blue items, making them blue/green for acoustic/electric.

It doesn't force just bi-secting sets. If you want, you can define all 4 sets to begin with, assign 4 colors, and go nuts.

### Storage Usage Helpers

#### Dupe Finding / Highlight

#### Folder Diffing + Merge

#### Usage Analyzer

#### In-Hand Duplication Detection

If two selected files share the same hash, it'll complain.

### Project Boxes

Folders, in a computer context, can really be two kinds of things: Boxes and organizational dividers.

What I mean is, when you start a new project - Maybe a lawyer getting a new client making a "Doe, John" folder or maybe a 3D artist making a "Dragon Scene" project - you make a folder first because you know it'll require more than one file.

To me, that's a *Box*. It's the big outer-most container for a project.

Within that box, you might have bags, folders, smaller-boxes, whatever, but they're deeply tied to that project's box.

With this understanding, you can make any folder a box. This does a few things:

* Creates a .fmproject
* Creates a README.md
* Creates a TODO.md
* Adds that .fmproject to a global project list

TODO: What all is stored in the .fmproject?

### .hide & .show

Sometimes the computer forces some folder layouts. Maybe a program only works if it's living in one particular location. You shouldn't have to visually sift though everything every time if there's a lot of junk in those locations you'll never care about.

This renders as a full solid background color change for that item, to make it very easy to see runs of items of the same marking in a folder.

.show can be used to reveal "dotfiles" (inc folders) that would otherwise be hidden without showing all of them.

### Remote Location Handling

The filesystem shouldn't be where you setup remote mounts. That's the operating system's job. 

But it should be aware of how to get good performance out of those locations. If you're going to run a big copy/delete job on a remote host to another location on that remote host, it shouldn't run on your local host. That's just silly.

### Terminal Opening

If browsing a remote, offer to open the terminal by SSH-ing to that remote, instead of just the local path. This also makes big NAS operations not depend on local staying up.

### Git

Git status, .gitignore

### Rename

Multi-file rename, but content aware via OCR/AI models

### Metadata Editing

Need to fix the genre on an audio file or remove the location data from an image? You can.

### Tranfer queue

Storage-aware - if it's disk A -> Disk B and an operation from disk C -> Disk D is started, both can run at the same time.

### Built in Editors

The existing small editors should be able to be invoked, but here they should default to saving to the directory they're opened to, asking if it should be a replace (save) or new-name (save-as)
