//! `get_object_alias` の文字列を読む（オブジェクト 1 つ分）。
//!
//! ```text
//! [Object]
//! frame=100,130,160,189      ← 開始・中間点・終了（シーン基準）
//! [Object.0]
//! effect.name=テキスト
//! サイズ=80.00
//! [Object.1]
//! effect.name=標準描画
//! X=100.00,200.00,300.00,400.00,直線移動,0
//! ```

#[derive(Debug, Clone, PartialEq)]
pub struct EffectAlias {
    pub name: String,
    /// 同じ名前の効果の中の順番（0 始まり）。API の `"効果名:n"` の n（ルール au2-rs-plugin）
    pub index: usize,
    pub items: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObjectAlias {
    /// 開始・中間点・終了のフレーム（シーン基準）
    pub frames: Vec<usize>,
    pub effects: Vec<EffectAlias>,
}

pub fn parse(alias: &str) -> ObjectAlias {
    let mut out = ObjectAlias::default();
    // None = [Object]、Some(i) = effects[i]
    let mut current: Option<Option<usize>> = None;
    for line in alias.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('[') && line.ends_with(']') {
            let head = &line[1..line.len() - 1];
            if head == "Object" {
                current = Some(None);
            } else if head.starts_with("Object.") {
                out.effects.push(EffectAlias { name: String::new(), index: 0, items: Vec::new() });
                current = Some(Some(out.effects.len() - 1));
            } else {
                current = None;
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match current {
            Some(None) if key == "frame" => {
                out.frames = value.split(',').filter_map(|v| v.trim().parse().ok()).collect();
            }
            Some(Some(i)) => {
                if key == "effect.name" {
                    let index = out.effects[..i].iter().filter(|e| e.name == value).count();
                    out.effects[i].name = value.to_string();
                    out.effects[i].index = index;
                } else {
                    out.effects[i].items.push((key.to_string(), value.to_string()));
                }
            }
            _ => {}
        }
    }
    out.effects.retain(|e| !e.name.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_probe_alias() {
        // 中間点プローブ_H（AI/devenv/probes/param_probe）が実機で取ったものの一部
        let a = parse(
            "[Object]\nframe=100,130,160,189\n[Object.0]\neffect.name=テキスト\nサイズ=80.00\nテキスト=line1\\nline2\n\
             [Object.1]\neffect.name=標準描画\nX=100.00,200.00,300.00,400.00,直線移動,0\nY=0.00\n\
             [Object.2]\neffect.name=ぼかし\n範囲=5\n[Object.3]\neffect.name=ぼかし\n範囲=1\n",
        );
        assert_eq!(a.frames, vec![100, 130, 160, 189]);
        assert_eq!(a.effects.len(), 4);
        assert_eq!(a.effects[1].name, "標準描画");
        assert_eq!(a.effects[1].items[0], ("X".into(), "100.00,200.00,300.00,400.00,直線移動,0".into()));
        assert_eq!((a.effects[2].name.as_str(), a.effects[2].index), ("ぼかし", 0));
        assert_eq!((a.effects[3].name.as_str(), a.effects[3].index), ("ぼかし", 1));
    }

    #[test]
    fn crlf_and_value_with_equals() {
        let a = parse("[Object]\r\nframe=0,59\r\n[Object.0]\r\neffect.name=テキスト\r\nテキスト=a=b\r\n");
        assert_eq!(a.frames, vec![0, 59]);
        assert_eq!(a.effects[0].items[0], ("テキスト".into(), "a=b".into()));
    }
}
