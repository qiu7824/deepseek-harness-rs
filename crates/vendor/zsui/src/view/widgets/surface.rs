/// A layout-time factory for responsive native surfaces and positioned children.
#[derive(Clone)]
pub struct SurfaceBuilder<Msg>(
    Arc<dyn Fn(Rect, Dpi) -> Vec<(crate::ZsCanvasRect, ViewNode<Msg>)> + Send + Sync>,
);

impl<Msg> fmt::Debug for SurfaceBuilder<Msg> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SurfaceBuilder")
    }
}

/// Builds children in local logical coordinates whenever the host size or DPI changes.
/// Children retain ordinary native input, accessibility, clipping and focus behavior.
pub fn surface<Msg>(
    builder: impl Fn(Rect, Dpi) -> Vec<(crate::ZsCanvasRect, ViewNode<Msg>)> + Send + Sync + 'static,
) -> ViewNode<Msg> {
    let mut node = ViewNode::new(ViewNodeKind::Spacer);
    node.surface_builder = Some(SurfaceBuilder(Arc::new(builder)));
    node
}

#[cfg(all(test, feature = "button"))]
mod surface_tests {
    use super::*;
    #[test]
    fn layout_reacts_to_resize_and_routes_native_children() {
        let mut view = surface(|bounds, dpi| {
            let w = bounds.width as f32 / dpi.scale_factor();
            vec![(
                crate::ZsCanvasRect::new(Dp(0.), Dp(0.), Dp(w / 2.), Dp(40.)),
                button("Action").id(WidgetId::new(123)).on_click(true),
            )]
        });
        for (w, scale) in [(400, 1.), (800, 2.)] {
            view.layout(&mut ViewLayoutCx::new(
                Rect {
                    x: 0,
                    y: 0,
                    width: w,
                    height: 200,
                },
                Dpi::new(scale * 96.),
            ));
            assert_eq!(view.children[0].bounds().unwrap().width, w / 2);
            let mut cx = ViewEventCx::new();
            view.event(
                &mut cx,
                &ViewEvent::Click {
                    widget: WidgetId::new(123),
                },
            );
            assert_eq!(cx.into_messages(), vec![true]);
        }
    }
}
