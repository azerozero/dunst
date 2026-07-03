use super::*;

#[derive(Default)]
pub(super) struct WalkState {
    pub(super) count: usize,
    pub(super) capped: bool,
}

pub(super) struct WalkAttributes {
    request: CFArray<CFString>,
}

pub(super) struct WalkContext {
    attrs: WalkAttributes,
    focus: FocusContext,
}

impl WalkContext {
    pub(super) fn new(app: &AxElement, window: &AxElement) -> Self {
        Self {
            attrs: WalkAttributes::new(),
            focus: FocusContext::from_app_window(app, window),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct FocusContext {
    focused_element: Option<ElementKey>,
    focused_window: Option<ElementKey>,
    target_window: Option<ElementKey>,
}

impl FocusContext {
    pub(super) fn from_app_window(app: &AxElement, window: &AxElement) -> Self {
        Self {
            focused_element: attr_ax_element(app, AX_FOCUSED_UI_ELEMENT_ATTRIBUTE)
                .and_then(|element| element_key(&element)),
            focused_window: attr_ax_element(app, AX_FOCUSED_WINDOW_ATTRIBUTE)
                .and_then(|element| element_key(&element)),
            target_window: element_key(window),
        }
    }

    fn element_focused(&self, element: &AxElement) -> bool {
        let key = element_key(element);
        reconciled_focus_key(key.as_ref(), self)
    }
}

impl WalkAttributes {
    pub(super) fn new() -> Self {
        Self {
            request: CFArray::from_CFTypes(&[
                CFString::new(kAXRoleAttribute),
                CFString::new(kAXValueAttribute),
                CFString::new(kAXTitleAttribute),
                CFString::new(kAXDescriptionAttribute),
                CFString::new(kAXHelpAttribute),
                CFString::new(kAXIdentifierAttribute),
                CFString::new(AX_FRAME_ATTRIBUTE),
                CFString::new(kAXPositionAttribute),
                CFString::new(kAXSizeAttribute),
                CFString::new(kAXEnabledAttribute),
                CFString::new(kAXChildrenAttribute),
                CFString::new(AX_MENU_ITEM_CMD_CHAR_ATTRIBUTE),
                CFString::new(AX_MENU_ITEM_CMD_MODIFIERS_ATTRIBUTE),
                CFString::new(AX_MENU_ITEM_CMD_VIRTUAL_KEY_ATTRIBUTE),
            ]),
        }
    }
}

pub(super) struct BatchValues {
    values: CFArray,
    len: usize,
}

impl BatchValues {
    pub(super) fn read(element: &AxElement, attrs: &WalkAttributes) -> Option<Self> {
        let mut values: CFArrayRef = ptr::null();
        // SAFETY: `element` and the CFArray of attribute names are valid;
        // `values` is a valid out-parameter and is checked before wrapping.
        let err = unsafe {
            AXUIElementCopyMultipleAttributeValues(
                element.as_ptr(),
                attrs.request.as_concrete_TypeRef(),
                0,
                &mut values,
            )
        };
        if err == kAXErrorSuccess && !values.is_null() {
            // SAFETY: AXUIElementCopyMultipleAttributeValues follows the
            // create rule on success, transferring a +1 CFArray to Rust.
            let values = unsafe { CFArray::wrap_under_create_rule(values) };
            // SAFETY: `values` is a valid CFArray just wrapped above.
            let len = unsafe { CFArrayGetCount(values.as_concrete_TypeRef()) as usize };
            Some(Self { values, len })
        } else {
            None
        }
    }

    pub(super) fn get(&self, index: usize) -> Option<CFTypeRef> {
        if index >= BATCH_ATTR_COUNT || index >= self.len {
            return None;
        }
        // SAFETY: index is bounds-checked against the CFArray count; the
        // returned item is borrowed and used only during this call path.
        let value = unsafe {
            CFArrayGetValueAtIndex(self.values.as_concrete_TypeRef(), index as isize) as CFTypeRef
        };
        normalize_batch_value(value)
    }
}

pub(super) struct NodeFields {
    ax_role: String,
    value: Option<String>,
    title: Option<String>,
    description: Option<String>,
    help: Option<String>,
    ax_identifier: Option<String>,
    cmd_char: Option<String>,
    cmd_modifiers: Option<u64>,
    cmd_virtual_key: Option<u16>,
    ax_actions: Vec<String>,
    frame: Option<Bbox>,
    enabled: bool,
    focused: bool,
}

pub(super) fn assemble_node(fields: NodeFields) -> RawAxNode {
    let label = fields
        .title
        .or(fields.description)
        .or_else(|| static_text_value_label(&fields.ax_role, &fields.value));

    RawAxNode {
        ax_role: fields.ax_role,
        label,
        help: fields.help,
        value: fields.value,
        ax_identifier: fields.ax_identifier,
        cmd_char: fields.cmd_char,
        cmd_modifiers: fields.cmd_modifiers,
        cmd_virtual_key: fields.cmd_virtual_key,
        ax_actions: fields.ax_actions,
        frame: fields.frame,
        enabled: fields.enabled,
        focused: fields.focused,
        children: Vec::new(),
    }
}

pub(super) fn shallow_raw_node(element: &AxElement) -> RawAxNode {
    let ax_role = attr_string(element, kAXRoleAttribute).unwrap_or_else(|| "AXUnknown".into());
    assemble_node(NodeFields {
        ax_role,
        value: attr_value_string(element, kAXValueAttribute),
        title: attr_label_string(element, kAXTitleAttribute),
        description: attr_label_string(element, kAXDescriptionAttribute),
        help: attr_string(element, kAXHelpAttribute),
        ax_identifier: attr_string(element, kAXIdentifierAttribute),
        cmd_char: attr_string(element, AX_MENU_ITEM_CMD_CHAR_ATTRIBUTE),
        cmd_modifiers: attr_number(element, AX_MENU_ITEM_CMD_MODIFIERS_ATTRIBUTE)
            .and_then(number_to_u64),
        cmd_virtual_key: attr_number(element, AX_MENU_ITEM_CMD_VIRTUAL_KEY_ATTRIBUTE)
            .and_then(number_to_u64)
            .and_then(|number| u16::try_from(number).ok()),
        ax_actions: read_node_actions(element),
        frame: frame(element),
        enabled: attr_bool(element, kAXEnabledAttribute).unwrap_or(true),
        focused: false,
    })
}

pub(super) fn static_text_value_label(ax_role: &str, value: &Option<String>) -> Option<String> {
    if ax_role == "AXStaticText" {
        value.clone().filter(|s| !s.is_empty())
    } else {
        None
    }
}

pub(super) fn walk_element(
    element: &AxElement,
    target_key: &TargetKey,
    depth: usize,
    state: &mut WalkState,
    ctx: &WalkContext,
) -> Result<RawAxNode> {
    state.count += 1;
    let Some(batch) = BatchValues::read(element, &ctx.attrs) else {
        return walk_element_single(element, target_key, depth, state, ctx);
    };
    let ax_role = batch
        .get(IDX_ROLE)
        .and_then(cf_string)
        .unwrap_or_else(|| "AXUnknown".into());

    let fields = NodeFields {
        ax_role,
        value: batch.get(IDX_VALUE).and_then(cf_value_string),
        title: batch.get(IDX_TITLE).and_then(cf_label_string),
        description: batch.get(IDX_DESCRIPTION).and_then(cf_label_string),
        help: batch.get(IDX_HELP).and_then(cf_string),
        ax_identifier: batch.get(IDX_IDENTIFIER).and_then(cf_string),
        cmd_char: batch.get(IDX_CMD_CHAR).and_then(cf_string),
        cmd_modifiers: batch.get(IDX_CMD_MODIFIERS).and_then(cf_u64),
        cmd_virtual_key: batch
            .get(IDX_CMD_VIRTUAL_KEY)
            .and_then(cf_u64)
            .and_then(|number| u16::try_from(number).ok()),
        ax_actions: read_node_actions(element),
        frame: frame_from_batch(&batch),
        enabled: batch.get(IDX_ENABLED).and_then(cf_bool).unwrap_or(true),
        focused: ctx.focus.element_focused(element),
    };
    finish_walk_element(
        element,
        target_key,
        depth,
        state,
        ctx,
        fields,
        batch.get(IDX_CHILDREN).and_then(cf_array),
    )
}

pub(super) fn walk_element_single(
    element: &AxElement,
    target_key: &TargetKey,
    depth: usize,
    state: &mut WalkState,
    ctx: &WalkContext,
) -> Result<RawAxNode> {
    let ax_role = attr_string(element, kAXRoleAttribute).unwrap_or_else(|| "AXUnknown".into());

    let fields = NodeFields {
        ax_role,
        value: attr_string(element, kAXValueAttribute),
        title: attr_label_string(element, kAXTitleAttribute),
        description: attr_label_string(element, kAXDescriptionAttribute),
        help: attr_string(element, kAXHelpAttribute),
        ax_identifier: attr_string(element, kAXIdentifierAttribute),
        cmd_char: attr_string(element, AX_MENU_ITEM_CMD_CHAR_ATTRIBUTE),
        cmd_modifiers: attr_number(element, AX_MENU_ITEM_CMD_MODIFIERS_ATTRIBUTE)
            .and_then(number_to_u64),
        cmd_virtual_key: attr_number(element, AX_MENU_ITEM_CMD_VIRTUAL_KEY_ATTRIBUTE)
            .and_then(number_to_u64)
            .and_then(|number| u16::try_from(number).ok()),
        ax_actions: read_node_actions(element),
        frame: frame(element),
        enabled: attr_bool(element, kAXEnabledAttribute).unwrap_or(true),
        focused: ctx.focus.element_focused(element),
    };
    finish_walk_element(
        element,
        target_key,
        depth,
        state,
        ctx,
        fields,
        attr_array(element, kAXChildrenAttribute),
    )
}

pub(super) fn finish_walk_element(
    element: &AxElement,
    target_key: &TargetKey,
    depth: usize,
    state: &mut WalkState,
    ctx: &WalkContext,
    fields: NodeFields,
    children: Option<CFArray>,
) -> Result<RawAxNode> {
    let mut node = assemble_node(fields);
    cache_element(target_key, &node, element);

    if depth >= max_depth() || state.count >= max_nodes() {
        state.capped = true;
        return Ok(node);
    }

    if let Some(children) = children {
        for child in ax_elements(&children) {
            if state.count >= max_nodes() {
                state.capped = true;
                break;
            }
            node.children
                .push(walk_element(&child, target_key, depth + 1, state, ctx)?);
        }
    }

    Ok(node)
}

pub(super) fn find_element(root: AxElement, wanted: &SceneNode) -> Result<Option<AxElement>> {
    let has_path = !wanted.path.is_empty();
    let require_path = has_path && collision_suffix_id(&wanted.id);
    let mut path_mismatch = false;
    let mut stack = vec![(root, 0usize, vec![0usize])];
    let mut seen = 0usize;

    while let Some((element, depth, path)) = stack.pop() {
        seen += 1;
        if seen > max_nodes() || depth > max_depth() {
            eprintln!("dunst-platform: live element search capped");
            break;
        }

        if has_path && path == wanted.path {
            if element_matches(&element, wanted) {
                return Ok(Some(element));
            }
            path_mismatch = true;
            if require_path {
                break;
            }
        }

        if !require_path && element_matches(&element, wanted) {
            return Ok(Some(element));
        }

        if let Some(children) = attr_array(&element, kAXChildrenAttribute) {
            let child_elements = ax_elements(&children);
            for (idx, child) in child_elements.into_iter().enumerate().rev() {
                let mut child_path = path.clone();
                child_path.push(idx);
                stack.push((child, depth + 1, child_path));
            }
        }
    }

    if require_path && path_mismatch {
        return Err(DunstError::ElementNotFound(format!(
            "id={} path={:?} resolved to a different live AX element",
            wanted.id, wanted.path
        )));
    }
    Ok(None)
}

pub(super) fn element_matches(element: &AxElement, wanted: &SceneNode) -> bool {
    element_key(element)
        .map(|key| key == ElementKey::from_scene(wanted))
        .unwrap_or(false)
}

pub(super) fn collision_suffix_id(id: &str) -> bool {
    let Some((_, suffix)) = id.rsplit_once('_') else {
        return false;
    };
    suffix.len() <= 3 && suffix.parse::<u32>().map(|n| n >= 2).unwrap_or(false)
}

pub(super) fn clear_cache() {
    AX_CACHE.with(|cache| cache.borrow_mut().clear());
}

pub(super) fn cache_element(target_key: &TargetKey, node: &RawAxNode, element: &AxElement) {
    AX_CACHE.with(|cache| {
        cache.borrow_mut().insert(
            CacheKey {
                target: target_key.clone(),
                element: ElementKey::from_raw(node),
            },
            element.retain_clone(),
        );
    });
}

pub(super) fn cached_element(key: &CacheKey) -> Option<AxElement> {
    AX_CACHE.with(|cache| cache.borrow().get(key).map(AxElement::retain_clone))
}

pub(super) fn remove_cached_element(key: &CacheKey) {
    AX_CACHE.with(|cache| {
        cache.borrow_mut().remove(key);
    });
}

pub(super) fn cached_element_matches_target(element: &AxElement, target: &Target) -> bool {
    target.window_id == 0 || ax_window_id(element) == Some(target.window_id)
}

pub(super) fn element_key(element: &AxElement) -> Option<ElementKey> {
    let ax_role = attr_string(element, kAXRoleAttribute).unwrap_or_else(|| "AXUnknown".into());
    let value = attr_string(element, kAXValueAttribute);
    let label = attr_label_string(element, kAXTitleAttribute)
        .or_else(|| attr_label_string(element, kAXDescriptionAttribute))
        .or_else(|| {
            if ax_role == "AXStaticText" {
                value.filter(|s| !s.is_empty())
            } else {
                None
            }
        });
    Some(ElementKey {
        ax_identifier: attr_string(element, kAXIdentifierAttribute),
        ax_role,
        label,
        bbox: frame(element).and_then(round_bbox),
    })
}

fn reconciled_focus_key(element: Option<&ElementKey>, focus: &FocusContext) -> bool {
    let Some(element) = element else {
        return false;
    };
    let Some(focused_element) = &focus.focused_element else {
        return false;
    };
    if let (Some(focused_window), Some(target_window)) =
        (&focus.focused_window, &focus.target_window)
    {
        if focused_window != target_window {
            return false;
        }
    }
    element == focused_element
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(ax_role: &str, label: &str, bbox: Option<(i64, i64, i64, i64)>) -> ElementKey {
        ElementKey {
            ax_identifier: None,
            ax_role: ax_role.to_string(),
            label: Some(label.to_string()),
            bbox,
        }
    }

    #[test]
    fn reconciled_focus_accepts_only_app_focused_element_in_target_window() {
        let target_window = key("AXWindow", "iTerm", Some((0, 0, 800, 600)));
        let other_window = key("AXWindow", "Other", Some((900, 0, 800, 600)));
        let focused = key("AXTextArea", "pane", Some((0, 0, 800, 300)));
        let sibling = key("AXTextArea", "pane", Some((0, 300, 800, 300)));

        let focus = FocusContext {
            focused_element: Some(focused.clone()),
            focused_window: Some(target_window.clone()),
            target_window: Some(target_window.clone()),
        };
        assert!(reconciled_focus_key(Some(&focused), &focus));
        assert!(!reconciled_focus_key(Some(&sibling), &focus));

        let wrong_window = FocusContext {
            focused_element: Some(focused.clone()),
            focused_window: Some(other_window),
            target_window: Some(target_window),
        };
        assert!(!reconciled_focus_key(Some(&focused), &wrong_window));
        assert!(!reconciled_focus_key(
            Some(&focused),
            &FocusContext::default()
        ));
    }
}
