# SmartConverter のスケルトン例（VapourSynth）
#
# オプション画面の［スクリプト］で拡張子（例：TS）にこのファイルを割り当てると、
# <%InputFile%> などを展開したスクリプトを一時ファイルとして作り、それをプリセットの入力にします。
# Windows のパスの \ がエスケープとして読まれないよう、パスは r"…" で囲みます。
# 読み込みには L-SMASH Works（vapoursynth-plugin-lsmashsource）を使います。作るスクリプトは常に UTF-8 です。
import vapoursynth as vs

core = vs.core
clip = core.lsmas.LWLibavSource(r"<%InputFile%>")
# 例：リサイズ
# clip = core.resize.Spline36(clip, 1280, 720)
clip.set_output()
