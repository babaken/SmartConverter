# SmartConverter のスケルトン例（AviSynth+）
#
# オプション画面の［スクリプト］で拡張子（例：TS）にこのファイルを割り当てると、
# <%InputFile%> などを展開したスクリプトを一時ファイルとして作り、それをプリセットの入力にします。
# 読み込みには L-SMASH Works（LWLibavVideoSource）を使います。Windows の DirectShowSource は Linux では使えません。
# 作るスクリプトは、Windows では Shift-JIS（AviSynth+ が読める形）、Linux では UTF-8 になります。

LWLibavVideoSource("<%InputFile%>")
AudioDub(last, LWLibavAudioSource("<%InputFile%>"))
# 例：インターレース解除とリサイズ
# Bob()
# Spline36Resize(1280, 720)
return last
