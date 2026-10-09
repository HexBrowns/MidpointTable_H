# MidpointTable_H（中間点の表）

フォーカス中のオブジェクトについて、行を点（開始・中間点・終了）、列をトラックバー項目にした表を出す AviUtl2 の汎用プラグイン。
セルを書き換えて「反映」を押すと、書き換えた列をまとめて書き込む。

- 仕様: `AI/specifications/20261008_MidpointTable_H_spec.md`
- 根拠にした実機の結果: `AI/docs/20261008_パラメータ編集_SDK実機確認.md`
- 本体 2.1.11 以降（aviutl2-rs 0.48）

## ビルドと配置

```bash
python AI/tools/au2_build.py MidpointTable_H                     # 本番へ
python AI/tools/au2_build.py MidpointTable_H --deploy-root=C:/ProgramData/aviutl2/AI/devenv/aviutl2/data   # devenv へ
```

## 作りの要点

| ファイル | 中身 |
|---|---|
| `src/track.rs` | 値の文字列（`値…,移動方法,設定`）の分解と組み立て、桁数、値の数を点の数に揃える |
| `src/alias.rs` | `get_object_alias` の読み取り（点のフレーム、効果ごとの項目） |
| `src/table.rs` | 表のモデル。未反映の管理、足す・掛ける・等間隔・同じ値・コピー・貼り付け |
| `src/edit_ops.rs` | 読み取り（`call_read_section`）と反映（`call_edit_section`）、中間点を足す・動かす・消す |
| `src/movement.rs` | 本体に登録されている移動方法の一覧（`.tra2` と組み込みから作る。`aviutl2.ini` は使わない） |
| `src/prefs.rs` | 隠す列・列の並び順・自動で読み直すか（`Plugin/MidpointTable_H/hidden_columns.txt`） |
| `src/gui.rs` | ウィンドウ（egui）と監視 |

- 監視（v0.5.0）: 表のオブジェクトのエイリアスは、本体のイベント（`UPDATE_OBJECT` / `CHANGE_FOCUS_OBJECT` / `CHANGE_EDIT_SCENE`）が来たときだけ読む（0.4 秒に 1 回まで）。
  フォーカスと区間の印は 1 秒ごとの軽い読み取りで追う。ウィンドウが見えていない（`IsWindowVisible` が偽）間は読みも描きもしない。
  表は見えている行だけを作る。経緯は `issues/20261010_heavy_with_many_midpoints_and_polls_while_hidden.md`

- 書き込みは「反映」ボタンからだけ。監視・フォーカスの変化からは書かない（本体の Undo を壊すため。ルール `au2-rs-plugin`）
- 値の数は点の数に揃えてから書く（本体は数を検査しない）
- 移動方法は自由入力にしない。移動なしの項目を移動ありにするときに選べるのは、直線移動と、表の他の列が使っている移動方法だけ
  （登録されていない名前を書くと本体が例外を出す）
- キー操作（Ctrl+C / Ctrl+V / 矢印 / Enter / F2 / Esc）は、セルをクリックしてフォーカスを置いている間だけ表が受ける。
  aviutl2-eframe は egui にフォーカスが無いときだけキーを本体へ渡すので、そうしないと本体のオブジェクトのコピー・貼り付けとして働く。
  Esc でフォーカスを外すと本体へ戻る
- セルの横ドラッグで値を増減する（1 px = 1、Shift で 1/10、Ctrl で 10 倍）。未反映の書き換えになるだけで、本体へは「反映」で書く
