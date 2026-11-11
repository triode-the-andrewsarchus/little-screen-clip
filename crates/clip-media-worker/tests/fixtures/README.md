`silence.aac` is a small generated stereo silence fixture, not a downloaded
recording. It tests compressed-audio probing without requiring an FFmpeg
executable during tests. Generation command:

```sh
ffmpeg -f lavfi -i anullsrc=r=44100:cl=stereo -t 0.05 -c:a aac -f adts silence.aac
```
