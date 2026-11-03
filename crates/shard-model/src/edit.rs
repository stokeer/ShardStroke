//! Describing and locating edits of a layer stack: human-readable names for history entries and lookup
//! of layers by their stable identity ([`Layer::uid`]).

use crate::{Layer, Mask};

/// Path (child indices from the root) of the layer with identity `uid`.
pub fn path_of_uid(layers: &[Layer], uid: u64) -> Option<Vec<usize>> {
    for (i, l) in layers.iter().enumerate() {
        if l.uid == uid {
            return Some(vec![i]);
        }
        if let Some(mut p) = path_of_uid(&l.children, uid) {
            p.insert(0, i);
            return Some(p);
        }
    }
    None
}

/// Identity of the layer at `path`.
pub fn uid_at(layers: &[Layer], path: &[usize]) -> Option<u64> {
    let (first, rest) = path.split_first()?;
    let l = layers.get(*first)?;
    if rest.is_empty() {
        Some(l.uid)
    } else {
        uid_at(&l.children, rest)
    }
}

/// Number of layers (groups included) in a stack.
pub fn count_layers(layers: &[Layer]) -> usize {
    layers.iter().map(|l| 1 + count_layers(&l.children)).sum()
}

fn flatten<'a>(layers: &'a [Layer], out: &mut Vec<&'a Layer>) {
    for l in layers {
        out.push(l);
        flatten(&l.children, out);
    }
}

fn mask_eq(a: &Option<Mask>, b: &Option<Mask>) -> bool {
    a == b
}

/// Name of the change that turned `before` into `after`, for the history ("Hide layer 'Dirt'").
pub fn describe_change(before: &[Layer], after: &[Layer]) -> String {
    let (mut b, mut a) = (Vec::new(), Vec::new());
    flatten(before, &mut b);
    flatten(after, &mut a);
    let in_before = |uid: u64| b.iter().any(|l| l.uid == uid);
    let in_after = |uid: u64| a.iter().any(|l| l.uid == uid);
    let added: Vec<&&Layer> = a.iter().filter(|l| !in_before(l.uid)).collect();
    let removed: Vec<&&Layer> = b.iter().filter(|l| !in_after(l.uid)).collect();
    let kind = |l: &Layer| if matches!(l.kind, crate::LayerKind::Group) { "group" } else { "layer" };
    match (added.len(), removed.len()) {
        (1, 0) => return format!("Add {} '{}'", kind(added[0]), added[0].name),
        (n, 0) if n > 1 => return format!("Add {n} layers"),
        (0, 1) => return format!("Remove {} '{}'", kind(removed[0]), removed[0].name),
        (0, n) if n > 1 => return format!("Remove {n} layers"),
        (x, y) if x > 0 && y > 0 => return "Replace layers".to_string(),
        _ => {}
    }
    // same set of layers: order, then per-layer fields
    if let Some(i) = (0..a.len()).find(|&i| b[i].uid != a[i].uid) {
        return format!("Move {} '{}'", kind(a[i]), a[i].name);
    }
    for (x, y) in b.iter().zip(a.iter()) {
        let n = &y.name;
        if x.name != y.name {
            return format!("Rename {} '{}' to '{}'", kind(y), x.name, y.name);
        }
        if x.visible != y.visible {
            return format!("{} {} '{}'", if y.visible { "Show" } else { "Hide" }, kind(y), n);
        }
        if x.blend != y.blend {
            return format!("Change blending of '{n}'");
        }
        if !mask_eq(&x.mask, &y.mask) {
            return match (&x.mask, &y.mask) {
                (None, Some(_)) => format!("Add mask to '{n}'"),
                (Some(m), Some(k)) if !m.explicit && k.explicit => format!("Add mask to '{n}'"),
                (Some(_), None) => format!("Remove mask from '{n}'"),
                (Some(m), Some(k)) if m.enabled != k.enabled => {
                    format!("{} mask of '{n}'", if k.enabled { "Enable" } else { "Disable" })
                }
                _ => format!("Edit mask of '{n}'"),
            };
        }
        if x.content != y.content {
            if x.content.len() == y.content.len() {
                if let Some((e, f)) = x.content.iter().zip(&y.content).find(|(e, f)| e != f) {
                    if e.visible != f.visible {
                        return format!("{} effect in '{n}'", if f.visible { "Show" } else { "Hide" });
                    }
                }
            }
            return format!("Edit '{n}'");
        }
    }
    "Edit layers".to_string()
}

// ---------------------------------------------------------------------------------------------- operations

/// Layer at `path`.
pub fn layer_at<'a>(layers: &'a [Layer], path: &[usize]) -> Option<&'a Layer> {
    let (first, rest) = path.split_first()?;
    let l = layers.get(*first)?;
    if rest.is_empty() {
        Some(l)
    } else {
        layer_at(&l.children, rest)
    }
}

/// Mutable layer at `path`.
pub fn layer_at_mut<'a>(layers: &'a mut [Layer], path: &[usize]) -> Option<&'a mut Layer> {
    let (first, rest) = path.split_first()?;
    let l = layers.get_mut(*first)?;
    if rest.is_empty() {
        Some(l)
    } else {
        layer_at_mut(&mut l.children, rest)
    }
}

/// The list of layers a path's parent holds (the root list for a one-element path).
fn siblings_mut<'a>(layers: &'a mut Vec<Layer>, parent: &[usize]) -> Option<&'a mut Vec<Layer>> {
    match parent.split_first() {
        None => Some(layers),
        Some((first, rest)) => siblings_mut(&mut layers.get_mut(*first)?.children, rest),
    }
}

/// Deep copy with fresh identities for the layer, its children and its effects (an effect id of 0 stays 0).
pub fn clone_fresh(l: &Layer) -> Layer {
    fn effects(es: &[crate::Effect]) -> Vec<crate::Effect> {
        es.iter()
            .map(|e| {
                let mut c = e.clone();
                if c.uid != 0 {
                    c.uid = crate::next_uid();
                }
                if let crate::EffectKind::Group { children } = &mut c.kind {
                    *children = effects(children);
                }
                c
            })
            .collect()
    }
    let mut c = l.clone();
    c.uid = crate::next_uid();
    c.content = effects(&l.content);
    if let Some(m) = &mut c.mask {
        m.effects = effects(&m.effects);
    }
    c.children = l.children.iter().map(clone_fresh).collect();
    c
}

/// Duplicate the layer at `path` right above the original (Painter: Ctrl+D). Returns the copy's path.
pub fn duplicate(layers: &mut Vec<Layer>, path: &[usize]) -> Option<Vec<usize>> {
    let (idx, parent) = path.split_last()?;
    let copy = clone_fresh(layer_at(layers, path)?);
    siblings_mut(layers, parent)?.insert(*idx, copy);
    Some(path.to_vec())
}

/// Remove and return the layer at `path`.
pub fn remove(layers: &mut Vec<Layer>, path: &[usize]) -> Option<Layer> {
    let (idx, parent) = path.split_last()?;
    let sib = siblings_mut(layers, parent)?;
    (*idx < sib.len()).then(|| sib.remove(*idx))
}

/// Insert `layer` as child number `index` of the group at `parent` (empty = top level). Returns its path.
pub fn insert(layers: &mut Vec<Layer>, parent: &[usize], index: usize, layer: Layer) -> Option<Vec<usize>> {
    let sib = siblings_mut(layers, parent)?;
    let i = index.min(sib.len());
    sib.insert(i, layer);
    let mut p = parent.to_vec();
    p.push(i);
    Some(p)
}

/// A new empty group.
pub fn new_group(name: &str, children: Vec<Layer>) -> Layer {
    Layer {
        uid: crate::next_uid(),
        kind: crate::LayerKind::Group,
        name: name.to_string(),
        visible: true,
        // Painter's groups are born Passthrough on every channel (a group channel with no entry is an
        // isolated Normal blend, see `shard_compose`)
        blend: vec![crate::ChannelBlend {
            mode: crate::BlendMode::PASSTHROUGH,
            channels: crate::ChannelSet(u128::MAX),
            opacity: 1.0,
        }],
        content: Vec::new(),
        mask: None,
        children,
    }
}

/// Wrap the layer at `path` into a new group (Painter: Ctrl+G). Returns the group's path.
pub fn group(layers: &mut Vec<Layer>, path: &[usize]) -> Option<Vec<usize>> {
    let l = remove(layers, path)?;
    let g = new_group("Group", vec![l]);
    let (idx, parent) = path.split_last()?;
    insert(layers, parent, *idx, g)
}

/// Replace the group at `path` by its children, in place. Returns the path of the first child.
pub fn ungroup(layers: &mut Vec<Layer>, path: &[usize]) -> Option<Vec<usize>> {
    let g = layer_at(layers, path)?;
    if g.kind != crate::LayerKind::Group {
        return None;
    }
    let g = remove(layers, path)?;
    let (idx, parent) = path.split_last()?;
    let sib = siblings_mut(layers, parent)?;
    for (k, child) in g.children.into_iter().enumerate() {
        sib.insert(*idx + k, child);
    }
    Some(path.to_vec())
}

/// Move the layer at `from` to become child `index` of the group at `parent` (paths refer to the stack
/// before the move). Moving a group into itself is refused. Returns the layer's new path.
pub fn move_to(layers: &mut Vec<Layer>, from: &[usize], parent: &[usize], index: usize) -> Option<Vec<usize>> {
    if parent.len() >= from.len() && parent[..from.len()] == *from {
        return None; // into itself or one of its descendants
    }
    let l = remove(layers, from)?;
    // removing an earlier sibling (or an earlier ancestor-level sibling) shifts the target path
    let mut parent = parent.to_vec();
    let mut index = index;
    let depth = from.len() - 1;
    if parent.len() >= depth && parent[..depth] == from[..depth] {
        if parent.len() == depth {
            if from[depth] < index {
                index -= 1;
            }
        } else if from[depth] < parent[depth] {
            parent[depth] -= 1;
        }
    }
    insert(layers, &parent, index, l)
}

/// Add a mask to the layer: white shows everything, black hides everything.
pub fn add_mask(l: &mut Layer, white: bool) {
    l.mask = Some(Mask { enabled: true, initial: if white { 1 } else { 0 }, effects: Vec::new(), explicit: true });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{next_uid, Layer, LayerKind};

    fn layer(name: &str) -> Layer {
        Layer {
            uid: next_uid(),
            kind: LayerKind::Layer,
            name: name.into(),
            visible: true,
            blend: vec![],
            content: vec![],
            mask: None,
            children: vec![],
        }
    }

    #[test]
    fn describes_common_edits() {
        let (a, b, c) = (layer("A"), layer("B"), layer("C"));
        let base = vec![a.clone(), b.clone()];
        let mut added = base.clone();
        added.insert(0, c.clone());
        assert_eq!(describe_change(&base, &added), "Add layer 'C'");
        assert_eq!(describe_change(&added, &base), "Remove layer 'C'");
        let mut hidden = base.clone();
        hidden[1].visible = false;
        assert_eq!(describe_change(&base, &hidden), "Hide layer 'B'");
        let swapped = vec![b.clone(), a.clone()];
        assert_eq!(describe_change(&base, &swapped), "Move layer 'B'");
        let mut renamed = base.clone();
        renamed[0].name = "Z".into();
        assert_eq!(describe_change(&base, &renamed), "Rename layer 'A' to 'Z'");
    }

    #[test]
    fn finds_layers_by_uid() {
        let mut g = layer("G");
        let child = layer("child");
        let uid = child.uid;
        g.children.push(child);
        let stack = vec![layer("top"), g];
        assert_eq!(path_of_uid(&stack, uid), Some(vec![1, 0]));
        assert_eq!(uid_at(&stack, &[1, 0]), Some(uid));
        assert_eq!(count_layers(&stack), 3);
    }

    #[test]
    fn shared_storage_is_not_copied_by_clone() {
        use crate::Shared;
        let a: Shared<Vec<u32>> = vec![1, 2, 3].into();
        let b = a.clone();
        assert!(a.ptr_eq(&b));
        let mut c = b.clone();
        c.make_mut().push(4);
        assert!(!c.ptr_eq(&a));
        assert_eq!(a.len(), 3);
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn duplicate_group_move_roundtrip() {
        let (a, b, c) = (layer("A"), layer("B"), layer("C"));
        let mut st = vec![a.clone(), b.clone(), c.clone()];
        // duplicate B: copy above, fresh uid
        let p = duplicate(&mut st, &[1]).unwrap();
        assert_eq!(p, vec![1]);
        assert_eq!(st.len(), 4);
        assert_eq!(st[1].name, "B");
        assert_ne!(st[1].uid, st[2].uid);
        // group the copy
        let g = group(&mut st, &[1]).unwrap();
        assert_eq!(st[g[0]].kind, crate::LayerKind::Group);
        assert_eq!(st[g[0]].children.len(), 1);
        // move C into the group at the end; earlier sibling removal shifts nothing here
        let np = move_to(&mut st, &[3], &[1], 1).unwrap();
        assert_eq!(np, vec![1, 1]);
        assert_eq!(st.len(), 3);
        assert_eq!(st[1].children.len(), 2);
        // moving a group into itself is refused
        assert!(move_to(&mut st, &[1], &[1], 0).is_none());
        // ungroup restores the flat list
        ungroup(&mut st, &[1]).unwrap();
        assert_eq!(st.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["A", "B", "C", "B"]);
    }

    #[test]
    fn move_down_within_a_list_accounts_for_the_removed_slot() {
        let mut st = vec![layer("A"), layer("B"), layer("C")];
        // drop A before the end (index 3): it lands last
        let p = move_to(&mut st, &[0], &[], 3).unwrap();
        assert_eq!(p, vec![2]);
        assert_eq!(st.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["B", "C", "A"]);
        // drop C above B (index 0)
        move_to(&mut st, &[1], &[], 0).unwrap();
        assert_eq!(st.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["C", "B", "A"]);
    }
}
