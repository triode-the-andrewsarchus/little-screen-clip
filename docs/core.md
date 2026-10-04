# Introduction

Hello, and welcome to my tool, **Little Screen Clip**.

Little Screen Clip aims to solve one core problem: Interacting with computers is too damn slow.

This means it has 5 main goals:
- Making it easier to transform data
- Making it easier to collect data
- Having an extremely fast to navigate UI
- Separating the things computers need from what humans want
- Saving of operation chains to avoid repeated work

To do this, we need to first recognize why these are problems

- Opening an image editor, audio editor, etc. takes time.
  - Most options are bloated, slow, legacy tools
- Most data people want is a subset of data they have, but filtering complex types is hard
  - Finding the differences in an image? text?
  - Pulling a section of audio out of a file
  - A page out of a PDF
  - A picture, with the background removed
  - Only the vocals in a song
- Some data is just annoying to get
  - Recording audio from your desktop is annoying if you just want a 3-second clip
- A lot of software is lazy and computer - not human - oriented
  - If you have a folder with one thing in it, why not just show the thing?!
- Files can belong to either users or the system, but we try to mix them.
  - This means users aren't in full control of file locations

## The Core Tools of the Desktop

To this end, we need to figure out what this actually means changing. It's easy to say "using computers is slow" but what tools can actually be improved?

I think there's 4 main tools that don't have good options yet:

* The Clipboard Manager
* The File Manager
* The Calculator
* The Application Launcher

Now, this may sound rude, after all, really smart people have worked really hard to make great versions of all of these.

However, they all share one fundamental flaw: They assume other applications will bear the brunt of the user's workload.

What I mean by this is developers, for damn good reason, avoid scope creep.

A clipboard manager needs to, at most, show your clipboard contents and let you navigate the history.
A file manager needs to show you files, let you navigate, copy + paste, etc.
You get the gist.

The problem is these programs are the core operating surface of the computer. I think the skeuomorphism paradigm that all desktops settled on in the late 90's naming changed the way we think about computer and the language informed the way programs are designed.

Back when these things were first made, we had the desktop - literally, the top of a desk. The place for you active files. Files - you know, pieces of paper. A file manager. A clipboard. These aren't some abstract computer words, they're hardware analogies.

The problem is, the words are wrong.

Calling what computers call a clipboard was the wrong metaphor, I think. Rather, it's your hand. It's what you're *holding*.

Calling the desktop the desktop just doesn't make sense when it's not some physical surface people interact in. But a modern file manager with previews? That's a lot closer. It's the surface where you open the folders and take a peek at what's inside.

Now, this sounds like I'm arguing semantics. We don't think about these things as their actual hardware, real world analogs anymore, we've abstracted them away. Copy-paste? Nobody thinks about actually copying a paper and using a glue to paste it on.

But the problem is those semantics inform developer direction. If you assume a file manager is a thing that manages files - a big cabinet with an organizational system - it sounds expressly out of scope for it to do anything else. But if it's your **desk**-**top**. The surface on which you modify, cut into, manipulate, sort, craft, and create on, the thing on which you use small tools to make things - then suddenly the scope is gigantic! It needs to do a little of everything. Sure, sometimes you might need to bust out a different tool for the job, you might have a dark room for photography, a workbench for some intricate trade - those are specific applications - but the core? The core is just you reaching for a pair of scissors or a sticker. Doing a quick change.  

If you view the calculator as a replacement for a desk adding machine, yeah, of course you'll get a number-pad with some math operations glued to it.

If you view the launcher (/start menu) as a place to only start applications - a big tool drawer - then that's all it is.

## Interconnected.

In the real world, if I'm going though files in a folder, TODO
