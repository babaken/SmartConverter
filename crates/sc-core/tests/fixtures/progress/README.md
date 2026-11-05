# 外部ツールの出力サンプル

進捗解析（`sc-core::progress`）のテスト用。ffmpeg の `testsrc2` で作った 5 分の動画（1280x720、29.97fps、MPEG-2 + MP2）を入力にして、
各ツールの標準出力と標準エラー出力をそのまま記録した（`\r` や色付けの制御文字を含む）。2026-11-02、Windows 11 で記録。
`qsvencc.txt` の OS・CPU・GPU の行は、記録した PC の詳しい情報を伏せている（進捗の解析には使わない）。

| ファイル | ツール | コマンド |
|---|---|---|
| `ffmpeg_video.txt` | ffmpeg 9.0.2 | `ffmpeg -y -i sample.ts -c:v libx264 -preset medium -an video.mp4` |
| `ffmpeg_audio.txt` | ffmpeg 9.0.2 | `ffmpeg -y -threads 0 -i sample.ts -vn -c:a pcm_s16le -ac 2 -ar 48000 -f wav audio.wav` |
| `ffmpeg_cut_rate.txt` | ffmpeg 9.0.2 | `ffmpeg -y -ss 30 -i sample.ts -t 120 -r 24000/1001 -c:v libx264 -preset medium -an cut.mp4` |
| `qsvencc.txt` | QSVEncC 8.35 (r4715) | `QSVEncC --avhw -i sample.ts -c hevc --profile main10 --output-res 640x480 --cbr 500 --tune vmaf -o out.hevc` |
| `mp4box.txt` | MP4Box (GPAC 26.07) | `MP4Box -add out.hevc -new out.mp4` |
| `opusenc.txt` | opusenc (opus-tools 0.2) | `opusenc audio.wav --bitrate 96 out.opus` |
