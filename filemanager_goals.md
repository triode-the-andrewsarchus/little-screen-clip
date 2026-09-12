The next phase of this project is a file manager application that integrates with the clipboard. Unlike the workbench UI, which is meant to be an ephermal popup for quick use, this is meant to fundamentally replace a normal file manager like Thunar or Nautilus.

Notably, I prefer the style of navigation like thunar's where typring letters immediately navigates to the match in the folder, not doing a recursive search or filtering

But with a few quirks.

First, we want to have a small preview of the existing clipboard slots. This will need a different render, as while on the big workbench it can take up ~half the screen, in the file manager it simply can't eat that much of the realestate. This should allow for dragging in a file/folder or selection of files/folders to a clipboard slot - fundamentally no different than hitting copy, except with the mouse action to drop it into a specific slot.

Second, it should be able to use all the same previews we've alredy made. This will allow for individual waveforms for audio files, picture thumbnails, midi thumbnails, etc.

Third, the existing small editors should be able to be invoked, but here they should default to saving to the directory they're opened to, asking if it should be a replace (save) or new-name (save-as)

Fourth, 