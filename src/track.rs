//! トラックバー項目の値の文字列を分けて組み立て直す。
//!
//! 形式（実機で確認。`AI/docs/20261008_パラメータ編集_SDK実機確認.md`）:
//! - 移動なし: `55.00`（値 1 つ）
//! - 移動あり: `値1,値2,…,値N,移動方法,設定`。値は中間点を含む各点に順に対応する（点の数 = 区間の数 + 1）
//!   - 例 `100.00,200.00,300.00,400.00,直線移動,0` / `0.00,-2000.00,カーブエディタ2@カーブエディタ2,0|2`
//!
//! 移動方法と設定は読んだものをそのまま書き戻す。本体は値の数を検査しない（足りなくても保存する）ので、
//! 数は組み立てる側で合わせる。
//!
//! 設定（中間点プローブ_H v0.2.0 の M1 で確認。devenv 2.1.11a）:
//! - **省略できない。** `…,400.00,直線移動` のように設定を書かないと、本体は `400.00` を移動方法の名前とみなして例外
//!   `not found movement` を出す（書き込みの処理ごと打ち切られる）。移動ありは必ず `,設定` まで書く
//! - `|` の前の数値はビットの組み合わせ: 1 = 加速、2 = 減速、4 = 中間点無視。8 は `8|` で保存される（時間制御の印とみられる）
//! - **中間点無視（4）のときは値が 2 つ（始点と終点）に切り詰められて保存される**

/// 分けた値
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub values: Vec<f64>,
    /// 移動方法。移動なしは `None`
    pub mode: Option<String>,
    /// 移動方法の後ろ（`0` や `0|2`）。カンマを含んでいてもそのまま持つ
    pub tail: Option<String>,
    /// 小数点以下の桁数（読んだ値の中で最も多いもの）
    pub decimals: usize,
}

fn decimals_of(s: &str) -> usize {
    s.split_once('.').map(|(_, d)| d.len()).unwrap_or(0)
}

/// 値の文字列を分ける。数値として読めない最初の要素を移動方法とみなす。
pub fn parse(raw: &str) -> Result<Track, String> {
    let parts: Vec<&str> = raw.split(',').collect();
    let mut values = Vec::new();
    let mut decimals = 0;
    let mut i = 0;
    while i < parts.len() {
        let p = parts[i].trim();
        match p.parse::<f64>() {
            Ok(v) if v.is_finite() => {
                values.push(v);
                decimals = decimals.max(decimals_of(p));
                i += 1;
            }
            _ => break,
        }
    }
    if values.is_empty() {
        return Err(format!("値が読めません: {raw}"));
    }
    let (mode, tail) = if i < parts.len() {
        let mode = parts[i].to_string();
        if mode.is_empty() {
            return Err(format!("移動方法が空です: {raw}"));
        }
        if i + 1 >= parts.len() {
            // 本体が書く形には必ず設定がある。無い形は書き戻すと例外になるので読むだけにする
            return Err(format!("移動方法の後ろに設定がありません: {raw}"));
        }
        (Some(mode), Some(parts[i + 1..].join(",")))
    } else {
        (None, None)
    };
    if mode.is_none() && values.len() != 1 {
        return Err(format!("移動方法が無いのに値が {} 個あります: {raw}", values.len()));
    }
    Ok(Track { values, mode, tail, decimals })
}

/// 値を書式に合わせて文字列にする（`-0.00` にはしない）
pub fn format_value(v: f64, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, v);
    if s.starts_with('-') && s[1..].chars().all(|c| c == '0' || c == '.') {
        s[1..].to_string()
    } else {
        s
    }
}

/// 組み立てる。`values` の数はそのまま使う（数を合わせるのは呼ぶ側。`fit`）。
/// 移動ありは設定を必ず付ける（無ければ `0`。省くと本体が例外を出す）
pub fn build(values: &[f64], decimals: usize, mode: Option<&str>, tail: Option<&str>) -> String {
    let vals: Vec<String> = values.iter().map(|v| format_value(*v, decimals)).collect();
    let mut s = vals.join(",");
    if let Some(m) = mode {
        s.push(',');
        s.push_str(m);
        s.push(',');
        s.push_str(tail.filter(|t| !t.is_empty()).unwrap_or("0"));
    }
    s
}

/// 設定の `|` の前の数値（1 = 加速、2 = 減速、4 = 中間点無視、8 = 時間制御とみられる）
pub fn flags_of(tail: Option<&str>) -> u32 {
    tail.and_then(|t| t.split('|').next()).and_then(|f| f.trim().parse().ok()).unwrap_or(0)
}

pub const FLAG_TWOPOINT: u32 = 4;

/// 値の数を点の数に合わせる。足りなければ最後の値で埋め、多ければ切る
pub fn fit(values: &[f64], points: usize) -> Vec<f64> {
    let mut v: Vec<f64> = values.iter().copied().take(points).collect();
    let last = v.last().copied().unwrap_or(0.0);
    while v.len() < points {
        v.push(last);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(raw: &str) {
        let t = parse(raw).unwrap();
        assert_eq!(build(&t.values, t.decimals, t.mode.as_deref(), t.tail.as_deref()), raw);
    }

    #[test]
    fn parse_and_build_roundtrip() {
        roundtrip("0.00");
        roundtrip("55.00");
        roundtrip("100.000");
        roundtrip("1");
        roundtrip("100.00,200.00,300.00,400.00,直線移動,0");
        roundtrip("-2600.00,2600.00,直線移動,0");
        roundtrip("0.00,-2000.00,カーブエディタ2@カーブエディタ2,0|2");
        roundtrip("0.00,1.00,4次式@Basic_S,0");
        // 実機で読んだ形（中間点プローブ_H v0.2.0 の M1）
        roundtrip("100.00,200.00,直線移動,4");
        roundtrip("100.00,200.00,300.00,400.00,直線移動,8|");
        roundtrip("0.00,1.00,移動無し,8|10^@min(音量/20-3,2)");
        // 本体の設定ダイアログで設定値を変えた形（設定値どうしはカンマ。中間点プローブ_H v0.2.3）
        roundtrip("100.00,200.00,300.00,400.00,プローブ移動_H,0|9,0,1");
        assert_eq!(parse("100.00,200.00,プローブ移動_H,0|9,0,1").unwrap().tail.as_deref(), Some("0|9,0,1"));
    }

    #[test]
    fn tail_is_mandatory() {
        // 設定の無い形は読むだけ（書き戻すと本体が例外を出す）
        assert!(parse("0.00,1.00,補間移動").is_err());
        // 組み立てでは必ず付ける
        assert_eq!(build(&[1.0, 2.0], 2, Some("直線移動"), None), "1.00,2.00,直線移動,0");
        assert_eq!(build(&[1.0, 2.0], 2, Some("直線移動"), Some("")), "1.00,2.00,直線移動,0");
        assert_eq!(flags_of(Some("4")), 4);
        assert_eq!(flags_of(Some("12|")), 12);
        assert_eq!(flags_of(Some("0|7|0|1")), 0);
        assert_eq!(flags_of(None), 0);
    }

    #[test]
    fn parse_fields() {
        let t = parse("100.00,200.00,300.00,400.00,直線移動,0").unwrap();
        assert_eq!(t.values, vec![100.0, 200.0, 300.0, 400.0]);
        assert_eq!(t.mode.as_deref(), Some("直線移動"));
        assert_eq!(t.tail.as_deref(), Some("0"));
        assert_eq!(t.decimals, 2);
        let t = parse("55.00").unwrap();
        assert_eq!(t.mode, None);
        assert_eq!(t.values, vec![55.0]);
        // 設定にカンマが入っていてもまとめて持つ
        let t = parse("1.0,2.0,何か@X,0,a,b").unwrap();
        assert_eq!(t.tail.as_deref(), Some("0,a,b"));
    }

    #[test]
    fn parse_errors() {
        assert!(parse("").is_err());
        assert!(parse("abc").is_err());
        assert!(parse("1.00,,0").is_err());
        assert!(parse("1.00,2.00").is_err());
        assert!(parse("1.00,2.00,直線移動").is_err());
    }

    #[test]
    fn decimals_and_negative_zero() {
        assert_eq!(format_value(-0.0001, 2), "0.00");
        assert_eq!(format_value(-1.5, 2), "-1.50");
        assert_eq!(format_value(12.3456, 3), "12.346");
        assert_eq!(format_value(7.6, 0), "8");
        assert_eq!(parse("1.5,2.125,直線移動,0").unwrap().decimals, 3);
    }

    #[test]
    fn fit_counts() {
        assert_eq!(fit(&[11.0, 22.0], 4), vec![11.0, 22.0, 22.0, 22.0]);
        assert_eq!(fit(&[1.0, 2.0, 3.0], 2), vec![1.0, 2.0]);
        assert_eq!(fit(&[5.0], 3), vec![5.0, 5.0, 5.0]);
    }
}
