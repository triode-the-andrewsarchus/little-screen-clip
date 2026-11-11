`with-cover.mp4` contains a generated 160 × 90 red video lasting 0.2 seconds
and a 32 × 32 blue attached JPEG. It verifies that metadata selects the video
rather than the artwork, and that shared worker/browser thumbnails distinguish
the red video from the blue cover. No downloaded media is included. Generation:

```sh
ffmpeg -f lavfi -i color=red:size=160x90:rate=10:duration=0.2 -c:v mpeg4 video.mp4
ffmpeg -f lavfi -i color=blue:size=32x32 -frames:v 1 cover.jpg
ffmpeg -i video.mp4 -i cover.jpg -map 0:v -map 1:v -c copy -disposition:v:1 attached_pic with-cover.mp4
```

`timed-colors.gif` is a generated 8 × 4 animation with red, green and blue
frames lasting 100, 400 and 500 ms respectively. It exercises fixed-rate
sampling and the final frame's hold duration without external media/tools.
Generated with Pillow's GIF writer (RGB solid-color frames, loop=0, disposal=2).
