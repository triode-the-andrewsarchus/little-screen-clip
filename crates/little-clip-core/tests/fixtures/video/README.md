`with-cover.mp4` contains a generated 160 × 90 red video lasting 0.2 seconds
and a 32 × 32 blue attached JPEG. It verifies that metadata selects the video
rather than the artwork. No downloaded media is included. Generation:

```sh
ffmpeg -f lavfi -i color=red:size=160x90:rate=10:duration=0.2 -c:v mpeg4 video.mp4
ffmpeg -f lavfi -i color=blue:size=32x32 -frames:v 1 cover.jpg
ffmpeg -i video.mp4 -i cover.jpg -map 0:v -map 1:v -c copy -disposition:v:1 attached_pic with-cover.mp4
```
