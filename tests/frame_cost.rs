//! 1 フレームの描画で表のモデルに掛かる時間の目安（実機の要望の実例: 点 1800・動く列 3・移動なしの列 13）。
//! `gui.rs` の `render_top` / `render_bottom` / `render_table` が 1 フレームに呼ぶ `Column` / `Table` の関数を、同じ回数だけ呼ぶ。
//! 時間は機械で変わるので、既定では走らせない:
//!   cargo test --release --test frame_cost -- --ignored --nocapture

use std::time::Instant;

use midpoint_table_h::table::{Column, Table};

fn sample_table(points: usize) -> Table {
    let frames: Vec<usize> = (0..points).collect();
    let moving = |base: f64, step: f64, decimals: usize| {
        let vals: Vec<String> = (0..points).map(|i| format!("{:.*}", decimals, base + step * i as f64)).collect();
        format!("{},直線移動,0", vals.join(","))
    };
    let mut columns = vec![
        Column::new("標準描画", 0, "X", &moving(-843.5, 0.7, 2), points),
        Column::new("標準描画", 0, "Y", &moving(-426.0, 0.4, 2), points),
    ];
    for item in ["Z", "中心X", "中心Y", "中心Z", "X軸回転", "Y軸回転", "Z軸回転", "拡大率", "縦横比", "透明度"] {
        columns.push(Column::new("標準描画", 0, item, "0.00", points));
    }
    columns.push(Column::new("リサイズ", 0, "拡大率", &moving(134.0, 0.01, 3), points));
    columns.push(Column::new("リサイズ", 0, "X", "100.000", points));
    columns.push(Column::new("リサイズ", 0, "Y", "100.000", points));
    columns.push(Column::new("図形", 0, "サイズ", "100", points));
    Table { frames, columns, checks: Vec::new(), selection: None }
}

/// `gui.rs` の 1 フレーム分（見えている行は `rows` 行）
fn one_frame(t: &Table, rows: usize) -> usize {
    let mut sink = 0;
    // render_top / render_bottom / ui / render_table の pending_total
    for _ in 0..4 {
        sink += t.pending_count();
    }
    for col in &t.columns {
        // 見出し（pending_here）・移動方法のコンボ・設定のボタンとホバー
        sink += col.pending_count();
        sink += usize::from(col.mode_changed() || col.needs_mode());
        sink += col.mode_detail().len() * 2;
        for _ in 0..4 {
            sink += col.effective_mode().map(|(m, _)| m.len()).unwrap_or(0);
        }
    }
    for r in 0..rows.min(t.points()) {
        for col in &t.columns {
            sink += usize::from(col.is_ignored_row(r));
            sink += usize::from(col.is_static() && !col.is_edited(r));
            sink += col.text(r).len();
            for _ in 0..3 {
                sink += usize::from(col.editable(r));
            }
        }
    }
    sink
}

#[test]
#[ignore]
fn frame_cost_with_1800_points() {
    let mut t = sample_table(1800);
    for (label, edit) in [("書き換えなし", false), ("1 列 1800 セルを書き換えた後", true)] {
        if edit {
            for r in 0..t.points() {
                t.set_cell(r, 0, r as f64);
            }
        }
        let started = Instant::now();
        let n = 10;
        let mut sink = 0;
        for _ in 0..n {
            sink += one_frame(&t, 50);
        }
        let per = started.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("[INFO] {label}: 1 フレーム {per:.2} ms（{sink}）");
    }
    // 移動なしの列の 1800 セルを範囲ごとドラッグしたときの 1 フレーム（`gui.rs` は掴んだ範囲の全部に `set_cell` する）
    let col = t.columns.iter().position(|c| c.is_static()).unwrap();
    let started = Instant::now();
    for r in 0..t.points() {
        t.set_cell(r, col, r as f64 * 0.5);
    }
    let sink = one_frame(&t, 50);
    println!("[INFO] 移動なしの列の 1800 セルをドラッグ: 1 フレーム {:.2} ms（{sink}）", started.elapsed().as_secs_f64() * 1000.0);
}
