// render() creates and mounts a root — mirrors the React 18 root API.

use wasm_bindgen::prelude::*;
use web_sys::{Element, Node};

use crate::vnode::{VNode, VNodeInner, Children};
use crate::diff::{diff_node, diff_children, unmount_vnode};
use crate::scheduler::{run_layout_effects, run_effects};

// ─── Root ───

pub struct Root {
    container: Element,
    root_vnode: Option<VNode>,
}

impl Root {
    pub fn new(container: Element) -> Self {
        Root { container, root_vnode: None }
    }

    pub fn render(&mut self, vnode: VNode) -> Result<(), JsValue> {
        let container_node: Node = self.container.clone().into();
        let ns = match self.container.namespace_uri().as_deref() {
            Some("http://www.w3.org/2000/svg") => "svg".to_string(),
            Some("http://www.w3.org/1998/Math/MathML") => "math".to_string(),
            _ => "html".to_string(),
        };

        // Wrap in a synthetic Fragment root
        let mut new_root = VNode {
            inner: VNodeInner::Fragment {
                children: Children(vec![vnode]),
                key: None,
            },
            original: crate::vnode::next_id(),
            _dom: None,
            _depth: 0,
            _index: 0,
            _flags: 0,
        };

        if let Some(old_root) = &self.root_vnode {
            // Update pass
            diff_node(&container_node, &mut new_root, Some(old_root), &ns)?;
        } else {
            // First mount: clear container and render fresh
            self.container.set_inner_html("");
            let mut children = match &new_root.inner {
                VNodeInner::Fragment { children, .. } => children.0.clone(),
                _ => vec![],
            };
            diff_children(&container_node, &mut children, &[], &ns, None)?;

            // Append any nodes that aren't yet in the DOM
            for child in &children {
                if let Some(dom) = &child._dom {
                    if dom.parent_node().is_none() {
                        container_node.append_child(dom)?;
                    }
                }
            }

            if let VNodeInner::Fragment { children: c, .. } = &mut new_root.inner {
                *c = Children(children);
            }
        }

        run_layout_effects();
        run_effects();

        self.root_vnode = Some(new_root);
        Ok(())
    }

    pub fn unmount(&mut self) {
        if let Some(root) = self.root_vnode.take() {
            unmount_vnode(&root, false);
            self.container.set_inner_html("");
        }
    }
}

/// Render `vnode` into `container`, replacing any existing content.
/// Returns the root handle for subsequent updates/unmounts.
pub fn render(vnode: VNode, container: Element) -> Result<Root, JsValue> {
    let mut root = Root::new(container);
    root.render(vnode)?;
    Ok(root)
}
