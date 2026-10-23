use crate::BlitzShellProvider;
use crate::convert_events::{
    color_scheme_to_theme, theme_to_color_scheme, winit_ime_to_blitz, winit_key_event_to_blitz,
    winit_modifiers_to_kbt_modifiers,
};
use crate::event::{BlitzShellEvent, create_waker};
use anyrender::WindowRenderer;
use blitz_dom::Document;
use blitz_paint::paint_scene;
use blitz_traits::events::{BlitzMouseButtonEvent, MouseEventButton, MouseEventButtons, UiEvent};
use blitz_traits::shell::Viewport;
use winit::keyboard::PhysicalKey;

use std::sync::Arc;
use std::task::Waker;
use std::time::Instant;
use winit::event::{ElementState, MouseButton};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::{Theme, WindowAttributes, WindowId};
use winit::{event::Modifiers, event::WindowEvent, keyboard::KeyCode, window::Window};

#[cfg(feature = "accessibility")]
use crate::accessibility::AccessibilityState;

pub struct WindowConfig<Rend: WindowRenderer> {
    doc: Box<dyn Document>,
    attributes: WindowAttributes,
    renderer: Rend,
}

impl<Rend: WindowRenderer> WindowConfig<Rend> {
    pub fn new(doc: Box<dyn Document>, renderer: Rend) -> Self {
        Self::with_attributes(doc, renderer, Window::default_attributes())
    }

    pub fn with_attributes(
        doc: Box<dyn Document>,
        renderer: Rend,
        attributes: WindowAttributes,
    ) -> Self {
        WindowConfig {
            doc,
            attributes,
            renderer,
        }
    }
}

pub struct View<Rend: WindowRenderer> {
    pub doc: Box<dyn Document>,

    pub renderer: Rend,
    pub waker: Option<Waker>,

    pub event_loop_proxy: EventLoopProxy<BlitzShellEvent>,
    pub window: Arc<Window>,

    /// The state of the keyboard modifiers (ctrl, shift, etc). Winit/Tao don't track these for us so we
    /// need to store them in order to have access to them when processing keypress events
    pub theme_override: Option<Theme>,
    pub keyboard_modifiers: Modifiers,
    pub buttons: MouseEventButtons,
    pub mouse_pos: (f32, f32),
    pub animation_timer: Option<Instant>,
    pub is_visible: bool,
    /// Sivryn patch: animated wheel scrolling and middle-click autoscroll.
    smooth_scroll: crate::scrolling::Smooth,
    autoscroll: Option<crate::scrolling::Autoscroll>,

    #[cfg(feature = "accessibility")]
    /// Accessibility adapter for `accesskit`.
    pub accessibility: AccessibilityState,
}

impl<Rend: WindowRenderer> View<Rend> {
    pub fn init(
        config: WindowConfig<Rend>,
        event_loop: &ActiveEventLoop,
        proxy: &EventLoopProxy<BlitzShellEvent>,
    ) -> Self {
        let winit_window = Arc::from(event_loop.create_window(config.attributes).unwrap());

        // TODO: make this conditional on text input focus
        winit_window.set_ime_allowed(true);

        // Create viewport
        let size = winit_window.inner_size();
        let scale = winit_window.scale_factor() as f32;
        let theme = winit_window.theme().unwrap_or(Theme::Light);
        let color_scheme = theme_to_color_scheme(theme);
        let viewport = Viewport::new(size.width, size.height, scale, color_scheme);

        // Create shell provider
        let shell_provider = BlitzShellProvider::new(winit_window.clone());

        let mut doc = config.doc;
        doc.set_viewport(viewport);
        doc.set_shell_provider(Arc::new(shell_provider));

        // If the document title is set prior to the window being created then it will
        // have been sent to a dummy ShellProvider and won't get picked up.
        // So we look for it here and set it if present.
        let title = doc.find_title_node().map(|node| node.text_content());
        if let Some(title) = title {
            winit_window.set_title(&title);
        }
        crate::canvas_frames::register(proxy.clone(), winit_window.id());
        crate::fullscreen::register(winit_window.clone());

        Self {
            renderer: config.renderer,
            waker: None,
            animation_timer: None,
            keyboard_modifiers: Default::default(),
            event_loop_proxy: proxy.clone(),
            window: winit_window.clone(),
            doc,
            theme_override: None,
            buttons: MouseEventButtons::None,
            mouse_pos: Default::default(),
            is_visible: winit_window.is_visible().unwrap_or(true),
            smooth_scroll: Default::default(),
            autoscroll: None,
            #[cfg(feature = "accessibility")]
            accessibility: AccessibilityState::new(&winit_window, proxy.clone()),
        }
    }

    pub fn replace_document(&mut self, new_doc: Box<dyn Document>, retain_scroll_position: bool) {
        let scroll = self.doc.viewport_scroll();
        let viewport = self.doc.viewport().clone();
        let shell_provider = self.doc.shell_provider.clone();

        self.doc = new_doc;
        self.doc.set_viewport(viewport);
        self.doc.set_shell_provider(shell_provider);
        self.poll();
        self.request_redraw();

        if retain_scroll_position {
            self.doc.set_viewport_scroll(scroll);
        }
    }

    pub fn theme_override(&self) -> Option<Theme> {
        self.theme_override
    }

    pub fn current_theme(&self) -> Theme {
        color_scheme_to_theme(self.doc.viewport().color_scheme)
    }

    pub fn set_theme_override(&mut self, theme: Option<Theme>) {
        self.theme_override = theme;
        let theme = theme.or(self.window.theme()).unwrap_or(Theme::Light);
        self.with_viewport(|v| v.color_scheme = theme_to_color_scheme(theme));
    }

    pub fn downcast_doc_mut<T: 'static>(&mut self) -> &mut T {
        self.doc.as_any_mut().downcast_mut::<T>().unwrap()
    }

    pub fn current_animation_time(&mut self) -> f64 {
        match &self.animation_timer {
            Some(start) => Instant::now().duration_since(*start).as_secs_f64(),
            None => {
                self.animation_timer = Some(Instant::now());
                0.0
            }
        }
    }
}

impl<Rend: WindowRenderer> View<Rend> {
    pub fn resume(&mut self) {
        // Resolve dom
        let animation_time = self.current_animation_time();
        self.doc.resolve(animation_time);

        // Resume renderer
        let (width, height) = self.doc.viewport().window_size;
        let scale = self.doc.viewport().scale_f64();
        self.renderer.resume(self.window.clone(), width, height);
        if !self.renderer.is_active() {
            panic!("Renderer failed to resume");
        };

        // Render
        self.renderer
            .render(|scene| paint_scene(scene, &self.doc, scale, width, height));

        // Set waker
        self.waker = Some(create_waker(&self.event_loop_proxy, self.window_id()));
    }

    pub fn suspend(&mut self) {
        self.waker = None;
        self.renderer.suspend();
    }

    pub fn poll(&mut self) -> bool {
        if crate::canvas_frames::take_pending() {
            self.request_redraw();
        }
        // Sivryn patch: Copy, Cut or Paste from the app's context menu.
        if let Some((node, edit)) = blitz_dom::input_editing::take_request() {
            self.run_edit(node, edit);
        }
        if self.doc.apply_focus_request() {
            self.request_redraw();
        }
        if let Some(waker) = &self.waker {
            let cx = std::task::Context::from_waker(waker);
            if self.doc.poll(Some(cx)) {
                #[cfg(feature = "accessibility")]
                {
                    if self.doc.has_changes() {
                        self.accessibility.update_tree(&self.doc);
                    }
                }

                self.request_redraw();
                return true;
            }
        }

        false
    }

    pub fn request_redraw(&self) {
        if self.renderer.is_active() {
            self.window.request_redraw();
        }
    }

    /// Sivryn patch: an edit on a text input, sent as its Ctrl key so the
    /// input's own handling (clipboard, input event) applies.
    fn run_edit(&mut self, node: usize, edit: blitz_dom::input_editing::Edit) {
        use blitz_traits::events::{BlitzKeyEvent, KeyState};
        use keyboard_types::{Code, Key, Location, Modifiers};
        if self.doc.get_node(node).is_none() {
            return;
        }
        self.doc.set_focus_to(node);
        let event = BlitzKeyEvent {
            key: Key::Character(edit.key().into()),
            code: Code::Unidentified,
            modifiers: Modifiers::CONTROL,
            location: Location::Standard,
            is_auto_repeating: false,
            is_composing: false,
            state: KeyState::Pressed,
            text: None,
        };
        self.doc.handle_ui_event(UiEvent::KeyDown(event));
        self.request_redraw();
    }

    /// Sivryn patch: applies pending wheel and autoscroll movement.
    fn apply_scrolling(&mut self) -> bool {
        let mut active = false;
        if let Some((node, dx, dy)) = self.smooth_scroll.step() {
            self.scroll(node, dx, dy);
            active |= self.smooth_scroll.is_active();
        }
        if let Some(auto) = self.autoscroll.as_mut() {
            let dy = auto.step(self.mouse_pos);
            let node = auto.node;
            if dy != 0.0 {
                self.scroll(node, 0.0, dy);
            }
            active = true;
        }
        active
    }

    fn scroll(&mut self, node: Option<usize>, dx: f64, dy: f64) {
        match node {
            Some(node) => self.doc.scroll_node_by(node, dx, dy),
            None => self.doc.scroll_viewport_by(dx, dy),
        }
    }

    fn stop_autoscroll(&mut self) {
        if self.autoscroll.take().is_some() {
            self.window.set_cursor(winit::window::CursorIcon::Default);
        }
    }

    pub fn redraw(&mut self) {
        let scrolling = self.apply_scrolling();
        let animation_time = self.current_animation_time();
        self.doc.resolve(animation_time);
        let (width, height) = self.doc.viewport().window_size;
        // Sivryn patch: a minimized window is 0×0 and its surface can't be
        // drawn to (wgpu panicked with "failed to get surface texture: Outdated").
        if width == 0 || height == 0 || self.window.is_minimized() == Some(true) {
            return;
        }
        let scale = self.doc.viewport().scale_f64();
        self.renderer
            .render(|scene| paint_scene(scene, &self.doc, scale, width, height));

        if self.is_visible && (self.doc.is_animating() || scrolling) {
            self.request_redraw();
        }
    }

    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }

    #[inline]
    pub fn with_viewport(&mut self, cb: impl FnOnce(&mut Viewport)) {
        let mut viewport = self.doc.viewport_mut();
        cb(&mut viewport);
        drop(viewport);
        let (width, height) = self.doc.viewport().window_size;
        if width > 0 && height > 0 {
            self.renderer.set_size(width, height);
            self.request_redraw();
        }
    }

    #[cfg(feature = "accessibility")]
    pub fn build_accessibility_tree(&mut self) {
        self.accessibility.update_tree(&self.doc);
    }

    pub fn handle_winit_event(&mut self, event: WindowEvent) {
        match event {
            // Window lifecycle events
            WindowEvent::Destroyed => {}
            WindowEvent::ActivationTokenDone { .. } => {},
            WindowEvent::CloseRequested => {
                // Currently handled at the level above in application.rs
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }

            // Window size/position events
            WindowEvent::Moved(_) => {}
            WindowEvent::Occluded(is_occluded) => {
                self.is_visible = !is_occluded;
                if self.is_visible {
                    self.request_redraw();
                }
            },
            WindowEvent::Resized(physical_size) => {
                self.with_viewport(|v| v.window_size = (physical_size.width, physical_size.height));
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.with_viewport(|v| v.set_hidpi_scale(scale_factor as f32));
            }

            // Theme events
            WindowEvent::ThemeChanged(theme) => {
                let color_scheme = theme_to_color_scheme(self.theme_override.unwrap_or(theme));
                self.doc.viewport_mut().color_scheme = color_scheme;
            }

            // Text / keyboard events
            WindowEvent::Ime(ime_event) => {
                self.doc.handle_ui_event(UiEvent::Ime(winit_ime_to_blitz(ime_event)));
                self.request_redraw();
            },
            WindowEvent::ModifiersChanged(new_state) => {
                // Store new keyboard modifier (ctrl, shift, etc) state for later use
                self.keyboard_modifiers = new_state;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(key_code) = event.physical_key else {
                    return;
                };

                // Sivryn patch: Escape goes to the app first (see escape.rs).
                if event.state.is_pressed() && key_code == KeyCode::Escape && crate::escape::escape() {
                    self.request_redraw();
                    return;
                }

                if event.state.is_pressed() {
                    let ctrl = self.keyboard_modifiers.state().control_key();
                    let meta = self.keyboard_modifiers.state().super_key();
                    let alt = self.keyboard_modifiers.state().alt_key();

                    // Ctrl/Super keyboard shortcuts
                    if ctrl | meta {
                        match key_code {
                            KeyCode::Equal => self.doc.viewport_mut().zoom_by(0.1),
                            KeyCode::Minus => self.doc.viewport_mut().zoom_by(-0.1),
                            KeyCode::Digit0 => self.doc.viewport_mut().set_zoom(1.0),
                            _ => {}
                        };
                    }

                    // Alt keyboard shortcuts
                    if alt {
                        match key_code {
                            KeyCode::KeyD => {
                                self.doc.devtools_mut().toggle_show_layout();
                                self.request_redraw();
                            }
                            KeyCode::KeyH => {
                                self.doc.devtools_mut().toggle_highlight_hover();
                                self.request_redraw();
                            }
                            KeyCode::KeyT => self.doc.print_taffy_tree(),
                            _ => {}
                        };
                    }

                }

                // Unmodified keypresses
                let key_event_data = winit_key_event_to_blitz(&event, self.keyboard_modifiers.state());
                let event = if event.state.is_pressed() {
                    UiEvent::KeyDown(key_event_data)
                } else {
                    UiEvent::KeyUp(key_event_data)
                };

                self.doc.handle_ui_event(event);
                self.request_redraw();
            }


            // Mouse/pointer events
            WindowEvent::CursorEntered { /*device_id*/.. } => {}
            WindowEvent::CursorLeft { /*device_id*/.. } => {}
            WindowEvent::CursorMoved { position, .. } => {
                let winit::dpi::LogicalPosition::<f32> { x, y } = position.to_logical(self.window.scale_factor());
                self.mouse_pos = (x, y);
                // Sivryn patch: zooming keeps the content under the mouse in place.
                blitz_dom::zoom_anchor::set_pointer(x as f64, y as f64);
                // Sivryn patch: scroll bars (blitz-dom's scrollbar.rs).
                let (consumed, repaint) = self.doc.scrollbar_move(x, y);
                if repaint {
                    self.request_redraw();
                }
                if consumed {
                    return;
                }
                let event = UiEvent::MouseMove(BlitzMouseButtonEvent {
                    x,
                    y,
                    button: Default::default(),
                    buttons: self.buttons,
                    mods: winit_modifiers_to_kbt_modifiers(self.keyboard_modifiers.state()),
                });
                self.doc.handle_ui_event(event);
                // Sivryn patch: keep the autoscroll cursor while moving.
                if self.autoscroll.is_some() {
                    self.window.set_cursor(winit::window::CursorIcon::AllScroll);
                }
            }
            // Sivryn patch: middle-click autoscroll (see scrolling.rs).
            WindowEvent::MouseInput { button: MouseButton::Middle, state, .. } => {
                match state {
                    ElementState::Pressed if self.autoscroll.is_some() => self.stop_autoscroll(),
                    ElementState::Pressed => {
                        let node = self.doc.get_hover_node_id();
                        self.autoscroll = Some(crate::scrolling::Autoscroll::new(self.mouse_pos, node));
                        self.window.set_cursor(winit::window::CursorIcon::AllScroll);
                        self.request_redraw();
                    }
                    ElementState::Released => {
                        if self.autoscroll.as_ref().is_some_and(|a| a.ends_on_release(self.mouse_pos)) {
                            self.stop_autoscroll();
                        }
                    }
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                let button = match button {
                    MouseButton::Left => MouseEventButton::Main,
                    MouseButton::Right => MouseEventButton::Secondary,
                    _ => return,
                };
                // Sivryn patch: a click ends autoscroll and does nothing else.
                if self.autoscroll.is_some() {
                    if state == ElementState::Pressed {
                        self.stop_autoscroll();
                    }
                    return;
                }

                // Sivryn patch: a press on a scroll bar drags it; the page sees neither
                // the press nor its release.
                if button == MouseEventButton::Main {
                    let (x, y) = self.mouse_pos;
                    let handled = match state {
                        ElementState::Pressed => self.doc.scrollbar_press(x, y),
                        ElementState::Released => self.doc.scrollbar_release(),
                    };
                    if handled {
                        self.request_redraw();
                        return;
                    }
                }

                match state {
                    ElementState::Pressed => self.buttons |= button.into(),
                    ElementState::Released => self.buttons ^= button.into(),
                }

                let event = BlitzMouseButtonEvent {
                    x: self.mouse_pos.0,
                    y: self.mouse_pos.1,
                    button,
                    buttons: self.buttons,
                    mods: winit_modifiers_to_kbt_modifiers(self.keyboard_modifiers.state()),
                };

                let event = match state {
                    ElementState::Pressed => UiEvent::MouseDown(event),
                    ElementState::Released => UiEvent::MouseUp(event),
                };
                self.doc.handle_ui_event(event);
                self.request_redraw();
            }
            // Sivryn patch: browser-like wheel scrolling and Ctrl+wheel for the
            // app (see scrolling.rs).
            WindowEvent::MouseWheel { delta, .. } => {
                if self.autoscroll.is_some() {
                    self.stop_autoscroll();
                }
                let notches = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y as f64,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y / crate::scrolling::NOTCH,
                };
                if self.keyboard_modifiers.state().control_key() && crate::scrolling::ctrl_wheel(notches) {
                    self.request_redraw();
                    return;
                }
                if !self.keyboard_modifiers.state().control_key() && crate::scrolling::wheel(notches) {
                    self.request_redraw();
                    return;
                }
                match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => {
                        let node = self.doc.get_hover_node_id();
                        let notch = crate::scrolling::NOTCH;
                        self.smooth_scroll.add(node, x as f64 * notch, y as f64 * notch);
                        self.request_redraw();
                    }
                    // Touchpads already deliver smooth pixel deltas.
                    winit::event::MouseScrollDelta::PixelDelta(offsets) => {
                        let has_changed = if let Some(hover_node_id) = self.doc.get_hover_node_id() {
                            self.doc.scroll_node_by_has_changed(hover_node_id, offsets.x, offsets.y)
                        } else {
                            self.doc.scroll_viewport_by_has_changed(offsets.x, offsets.y)
                        };
                        if has_changed {
                            self.request_redraw();
                        }
                    }
                }
            }

            // File events
            // Sivryn patch: forward file drag-and-drop (see file_drop.rs).
            WindowEvent::DroppedFile(path) => crate::file_drop::emit(crate::file_drop::FileDrop::Dropped(path)),
            WindowEvent::HoveredFile(path) => crate::file_drop::emit(crate::file_drop::FileDrop::Hovered(path)),
            WindowEvent::HoveredFileCancelled => crate::file_drop::emit(crate::file_drop::FileDrop::Cancelled),
            WindowEvent::Focused(_) => {}

            // Touch and motion events
            // Todo implement touch scrolling
            WindowEvent::Touch(_) => {}
            WindowEvent::TouchpadPressure { .. } => {}
            WindowEvent::AxisMotion { .. } => {}
            WindowEvent::PinchGesture { .. } => {},
            WindowEvent::PanGesture { .. } => {},
            WindowEvent::DoubleTapGesture { .. } => {},
            WindowEvent::RotationGesture { .. } => {},
        }
    }
}
