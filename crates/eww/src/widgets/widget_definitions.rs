#![allow(clippy::option_map_unit_fn)]
use super::{build_widget::BuilderArgs, circular_progressbar::*, run_command, transform::*};
use crate::{
    def_widget, enum_parse, error_handling_ctx,
    util::{self, list_difference},
    widgets::{build_widget::build_gtk_widget, systray},
};
use anyhow::{anyhow, Context, Result};
use codespan_reporting::diagnostic::Severity;
use eww_shared_util::Spanned;

use gdk::{EventKey, ModifierType, NotifyType};
use glib::translate::FromGlib;
use gtk::{self, glib, prelude::*, DestDefaults, TargetEntry, TargetList};
use gtk::{cairo, gdk, pango};
use itertools::Itertools;

use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Duration,
};
use yuck::{
    config::file_provider::YuckFileProvider,
    error::{DiagError, DiagResult},
    format_diagnostic::{span_to_secondary_label, DiagnosticExt},
    gen_diagnostic,
    parser::from_ast::FromAst,
};

/// Connect a gtk signal handler inside of this macro to ensure that when the same code gets run multiple times,
/// the previously connected singal handler first gets disconnected.
/// Can take an optional condition.
/// If the condition is false, we disconnect the handler without running the connect_expr,
/// thus not connecting a new handler unless the condition is met.
macro_rules! connect_signal_handler {
    ($widget:ident, if $cond:expr, $connect_expr:expr) => {{
        const KEY:&str = std::concat!("signal-handler:", std::line!());
        unsafe {
            let old = $widget.data::<gtk::glib::SignalHandlerId>(KEY);

            if let Some(old) = old {
                 let a = old.as_ref().as_raw();
                 $widget.disconnect(gtk::glib::SignalHandlerId::from_glib(a));
            }

            $widget.set_data::<gtk::glib::SignalHandlerId>(KEY, $connect_expr);
        }
    }};
    ($widget:ident, $connect_expr:expr) => {{
        connect_signal_handler!($widget, if true, $connect_expr)
    }};
}

// TODO figure out how to
// TODO https://developer.gnome.org/gtk3/stable/GtkFixed.html

pub const BUILTIN_WIDGET_NAMES: &[&str] = &[
    WIDGET_NAME_BOX,
    WIDGET_NAME_CENTERBOX,
    WIDGET_NAME_EVENTBOX,
    WIDGET_NAME_TOOLTIP,
    WIDGET_NAME_CIRCULAR_PROGRESS,
    WIDGET_NAME_GRAPH,
    WIDGET_NAME_TRANSFORM,
    WIDGET_NAME_SCALE,
    WIDGET_NAME_PROGRESS,
    WIDGET_NAME_IMAGE,
    WIDGET_NAME_BUTTON,
    WIDGET_NAME_LABEL,
    WIDGET_NAME_LITERAL,
    WIDGET_NAME_INPUT,
    WIDGET_NAME_CALENDAR,
    WIDGET_NAME_COLOR_BUTTON,
    WIDGET_NAME_EXPANDER,
    WIDGET_NAME_COLOR_CHOOSER,
    WIDGET_NAME_COMBO_BOX_TEXT,
    WIDGET_NAME_CHECKBOX,
    WIDGET_NAME_REVEALER,
    WIDGET_NAME_SCROLL,
    WIDGET_NAME_LISTBOX,
    WIDGET_NAME_LISTROW,
    WIDGET_NAME_MENU,
    WIDGET_NAME_MENU_OPTION,
    WIDGET_NAME_OVERLAY,
    WIDGET_NAME_STACK,
    WIDGET_NAME_SYSTRAY,
    WIDGET_NAME_DRAWINGAREA,
];

/// widget definitions
pub(super) fn widget_use_to_gtk_widget(bargs: &mut BuilderArgs) -> Result<gtk::Widget> {
    let gtk_widget = match bargs.widget_use.name.as_str() {
        WIDGET_NAME_BOX => build_gtk_box(bargs)?.upcast(),
        WIDGET_NAME_CENTERBOX => build_center_box(bargs)?.upcast(),
        WIDGET_NAME_EVENTBOX => build_gtk_event_box(bargs)?.upcast(),
        WIDGET_NAME_TOOLTIP => build_tooltip(bargs)?.upcast(),
        WIDGET_NAME_CIRCULAR_PROGRESS => build_circular_progress_bar(bargs)?.upcast(),
        WIDGET_NAME_GRAPH => build_graph(bargs)?.upcast(),
        WIDGET_NAME_TRANSFORM => build_transform(bargs)?.upcast(),
        WIDGET_NAME_SCALE => build_gtk_scale(bargs)?.upcast(),
        WIDGET_NAME_PROGRESS => build_gtk_progress(bargs)?.upcast(),
        WIDGET_NAME_IMAGE => build_gtk_image(bargs)?.upcast(),
        WIDGET_NAME_BUTTON => build_gtk_button(bargs)?.upcast(),
        WIDGET_NAME_LABEL => build_gtk_label(bargs)?.upcast(),
        WIDGET_NAME_LITERAL => build_gtk_literal(bargs)?.upcast(),
        WIDGET_NAME_INPUT => build_gtk_input(bargs)?.upcast(),
        WIDGET_NAME_CALENDAR => build_gtk_calendar(bargs)?.upcast(),
        WIDGET_NAME_COLOR_BUTTON => build_gtk_color_button(bargs)?.upcast(),
        WIDGET_NAME_EXPANDER => build_gtk_expander(bargs)?.upcast(),
        WIDGET_NAME_COLOR_CHOOSER => build_gtk_color_chooser(bargs)?.upcast(),
        WIDGET_NAME_COMBO_BOX_TEXT => build_gtk_combo_box_text(bargs)?.upcast(),
        WIDGET_NAME_CHECKBOX => build_gtk_checkbox(bargs)?.upcast(),
        WIDGET_NAME_REVEALER => build_gtk_revealer(bargs)?.upcast(),
        WIDGET_NAME_SCROLL => build_gtk_scrolledwindow(bargs)?.upcast(),
        WIDGET_NAME_LISTROW => build_gtk_listrow(bargs)?.upcast(),
        WIDGET_NAME_LISTBOX => build_gtk_listbox(bargs)?.upcast(),
        WIDGET_NAME_MENU_OPTION => build_gtk_menu_option(bargs)?.upcast(),
        WIDGET_NAME_MENU => build_gtk_menu(bargs)?.upcast(),
        WIDGET_NAME_OVERLAY => build_gtk_overlay(bargs)?.upcast(),
        WIDGET_NAME_STACK => build_gtk_stack(bargs)?.upcast(),
        WIDGET_NAME_SYSTRAY => build_systray(bargs)?.upcast(),
        WIDGET_NAME_DRAWINGAREA => build_gtk_drawingarea(bargs)?.upcast(),
        _ => {
            return Err(DiagError(gen_diagnostic! {
                msg = format!("referenced unknown widget `{}`", bargs.widget_use.name),
                label = bargs.widget_use.name_span => "Used here",
            })
            .into())
        }
    };
    Ok(gtk_widget)
}

/// Deprecated attributes from top of widget hierarchy
static DEPRECATED_ATTRS: &[&str] = &["timeout", "onscroll", "onhover", "cursor"];

/// attributes that apply to all widgets
/// @widget widget
/// @desc these properties apply to _all_ widgets, and can be used anywhere!
pub(super) fn resolve_widget_attrs(bargs: &mut BuilderArgs, gtk_widget: &gtk::Widget) -> Result<()> {
    let contained_deprecated: Vec<_> = DEPRECATED_ATTRS.iter().filter_map(|x| bargs.unhandled_attrs.remove_entry(*x)).collect();
    if !contained_deprecated.is_empty() {
        let diag = error_handling_ctx::stringify_diagnostic(gen_diagnostic! {
            kind =  Severity::Error,
            msg = "Unsupported attributes provided",
            label = bargs.widget_use.span => "Found in here",
            note = format!(
                "The attribute(s) ({}) has/have been removed, as GTK does not support it consistently. Instead, use eventbox to wrap this widget and set the attribute there. See #251 (https://github.com/elkowar/eww/issues/251) for more details.",
                contained_deprecated.iter().map(|(x, _)| x).join(", ")
            ),
        }).unwrap();
        eprintln!("{}", diag);
    }

    let css_provider = gtk::CssProvider::new();
    let css_provider2 = css_provider.clone();

    let visible_result: Result<_> = (|| {
        let visible_expr = bargs.widget_use.attrs.attrs.get("visible").map(|x| x.value.as_simplexpr()).transpose()?;
        if let Some(visible_expr) = visible_expr {
            let visible = bargs.scope_graph.evaluate_simplexpr_in_scope(bargs.calling_scope, &visible_expr)?.as_bool()?;
            connect_first_map(gtk_widget, move |w| {
                if visible {
                    w.show();
                } else {
                    w.hide();
                }
            });
        }
        Ok(())
    })();
    if let Err(err) = visible_result {
        error_handling_ctx::print_error(err);
    }

    def_widget!(bargs, _g, gtk_widget, {
        // @prop class - css class name
        prop(class: as_string) {
            // TODO currently this overrides classes that gtk adds automatically, which is kinda stupid...
            let old_classes = gtk_widget.style_context().list_classes();
            let old_classes = old_classes.iter().map(|x| x.as_str()).collect::<Vec<&str>>();
            let new_classes = class.split(' ').collect::<Vec<_>>();
            let (missing, new) = list_difference(&old_classes, &new_classes);
            for class in missing {
                gtk_widget.style_context().remove_class(class);
            }
            for class in new {
                gtk_widget.style_context().add_class(class);
            }
        },
        // @prop valign - how to align this vertically. possible values: $alignment
        prop(valign: as_string) { gtk_widget.set_valign(parse_align(&valign)?) },
        // @prop halign - how to align this horizontally. possible values: $alignment
        prop(halign: as_string) { gtk_widget.set_halign(parse_align(&halign)?) },
        // @prop vexpand - should this container expand vertically. Default: false.
        prop(vexpand: as_bool = false) { gtk_widget.set_vexpand(vexpand) },
        // @prop hexpand - should this widget expand horizontally. Default: false.
        prop(hexpand: as_bool = false) { gtk_widget.set_hexpand(hexpand) },
        // @prop width - width of this element. note that this can not restrict the size if the contents stretch it
        // @prop height - height of this element. note that this can not restrict the size if the contents stretch it
        prop(width: as_i32?, height: as_i32?) {
            gtk_widget.set_size_request(
                width.unwrap_or_else(|| gtk_widget.allocated_width()),
                height.unwrap_or_else(|| gtk_widget.allocated_height())
            );
        },
        // @prop active - If this widget can be interacted with
        prop(active: as_bool = true) { gtk_widget.set_sensitive(active) },
        // @prop tooltip - tooltip text (on hover)
        prop(tooltip: as_string) {
            gtk_widget.set_tooltip_text(Some(&tooltip));
        },
        // @prop visible - visibility of the widget
        prop(visible: as_bool = true) {
            if visible { gtk_widget.show(); } else { gtk_widget.hide(); }
        },
        // @prop style - inline scss style applied to the widget
        prop(style: as_string) {
            gtk_widget.reset_style();
            css_provider.load_from_data(grass::from_string(format!("* {{ {} }}", style), &grass::Options::default())?.as_bytes())?;
            gtk_widget.style_context().add_provider(&css_provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION)
        },
        // @prop css - scss code applied to the widget, i.e.: `button {color: red;}`
        prop(css: as_string) {
            gtk_widget.reset_style();
            css_provider2.load_from_data(grass::from_string(css, &grass::Options::default())?.as_bytes())?;
            gtk_widget.style_context().add_provider(&css_provider2, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION)
        },
        prop(menu: as_string) {
            if let Ok(items) = serde_json::from_str::<Vec<ContextMenu>>(&menu) {
                let context_menu = build_context_menu(items);
                let context_menu2 = context_menu.clone();

                gtk_widget.add_events(gdk::EventMask::KEY_PRESS_MASK);
                gtk_widget.add_events(gdk::EventMask::BUTTON_PRESS_MASK);

                connect_signal_handler!(
                    gtk_widget,
                    gtk_widget.connect_key_press_event(move |_, evt| {
                        match evt.keyval().name() {
                            Some(name) if name.as_str() == "Menu" => context_menu.popup_at_pointer(Some(evt)),
                            _ => {}
                        };

                        glib::Propagation::Proceed
                    })
                );
                connect_signal_handler!(
                    gtk_widget,
                    gtk_widget.connect_button_press_event(move |_, evt| {
                        match evt.button() {
                            1 => context_menu2.clone().popdown(),
                            3 => context_menu2.clone().popup_at_pointer(Some(evt)),
                            _ => {}
                        };

                        glib::Propagation::Proceed
                    })
                );
            }
        }
    });
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct ContextMenu {
    label: String,
    action: Option<String>,
    #[serde(default)]
    timeout: Timeout,
    submenu: Option<Vec<ContextMenu>>,
}

#[derive(Debug, serde::Deserialize)]
struct Timeout(Duration);
impl Default for Timeout {
    fn default() -> Timeout {
        Timeout(Duration::from_millis(200))
    }
}

fn build_context_menu(items: Vec<ContextMenu>) -> gtk::Menu {
    let context_menu = gtk::Menu::new();

    for item in items {
        let menu_item = gtk::MenuItem::with_label(&item.label);

        if let Some(action) = item.action {
            menu_item.connect_activate(move |_| {
                run_command(item.timeout.0, &action, &[] as &[&str]);
            });
        }

        if let Some(submenu) = item.submenu {
            let menu = build_context_menu(submenu);
            menu_item.set_submenu(Some(&menu));
        }

        context_menu.append(&menu_item);
    }

    context_menu.show_all();

    context_menu
}

/// @widget !range
pub(super) fn resolve_range_attrs(bargs: &mut BuilderArgs, gtk_widget: &gtk::Range) -> Result<()> {
    gtk_widget.set_sensitive(false);

    // only allow changing the value via the value property if the user isn't currently dragging
    let is_being_dragged = Rc::new(RefCell::new(false));
    gtk_widget.connect_button_press_event(glib::clone!(@strong is_being_dragged => move |_, _| {
        *is_being_dragged.borrow_mut() = true;
        glib::Propagation::Proceed
    }));
    gtk_widget.connect_button_release_event(glib::clone!(@strong is_being_dragged => move |_, _| {
        *is_being_dragged.borrow_mut() = false;
        glib::Propagation::Proceed
    }));

    // We keep track of the last value that has been set via gtk_widget.set_value (by a change in the value property).
    // We do this so we can detect if the new value came from a scripted change or from a user input from within the value_changed handler
    // and only run on_change when it's caused by manual user input
    let last_set_value = Rc::new(RefCell::new(None));
    let last_set_value_clone = last_set_value.clone();

    def_widget!(bargs, _g, gtk_widget, {
        // @prop value - the value
        prop(value: as_f64) {
            if !*is_being_dragged.borrow() {
                *last_set_value.borrow_mut() = Some(value);
                gtk_widget.set_value(value);
            }
        },
        // @prop min - the minimum value
        prop(min: as_f64) { gtk_widget.adjustment().set_lower(min)},
        // @prop max - the maximum value
        prop(max: as_f64) { gtk_widget.adjustment().set_upper(max)},
        // @prop timeout - timeout of the command. Default: "200ms"
        // @prop onchange - command executed once the value is changes. The placeholder `{}`, used in the command will be replaced by the new value.
        prop(timeout: as_duration = Duration::from_millis(200), onchange: as_string) {
            gtk_widget.set_sensitive(true);
            gtk_widget.add_events(gdk::EventMask::PROPERTY_CHANGE_MASK);
            let last_set_value = last_set_value_clone.clone();
            connect_signal_handler!(gtk_widget, gtk_widget.connect_value_changed(move |gtk_widget| {
                let value = gtk_widget.value();
                if last_set_value.borrow_mut().take() != Some(value) {
                    run_command(timeout, &onchange, &[value]);
                }
            }));
        }
    });
    Ok(())
}

/// @widget !orientable
pub(super) fn resolve_orientable_attrs(bargs: &mut BuilderArgs, gtk_widget: &gtk::Range) -> Result<()> {
    def_widget!(bargs, _g, gtk_widget, {
        // @prop orientation - orientation of the widget. Possible values: $orientation
        prop(orientation: as_string) { gtk_widget.set_orientation(parse_orientation(&orientation)?) },
    });
    Ok(())
}

// concrete widgets

const WIDGET_NAME_COMBO_BOX_TEXT: &str = "combo-box-text";
/// @widget combo-box-text
/// @desc A combo box allowing the user to choose between several items.
fn build_gtk_combo_box_text(bargs: &mut BuilderArgs) -> Result<gtk::ComboBoxText> {
    let gtk_widget = gtk::ComboBoxText::new();
    def_widget!(bargs, _g, gtk_widget, {
        // @prop items - Items that should be displayed in the combo box
        prop(items: as_vec) {
            gtk_widget.remove_all();
            for i in items {
                gtk_widget.append_text(&i);
            }
        },
        // @prop timeout - timeout of the command: Default: "200ms"
        // @prop onchange - runs the code when a item was selected, replacing {} with the item as a string
        prop(timeout: as_duration = Duration::from_millis(200), onchange: as_string) {
            connect_signal_handler!(gtk_widget, gtk_widget.connect_changed(move |gtk_widget| {
                run_command(timeout, &onchange, &[gtk_widget.active_text().unwrap_or_else(|| "".into())]);
            }));
        },
    });
    Ok(gtk_widget)
}

const WIDGET_NAME_EXPANDER: &str = "expander";
/// @widget expander
/// @desc A widget that can expand and collapse, showing/hiding it's children. Should contain
/// exactly one child.
fn build_gtk_expander(bargs: &mut BuilderArgs) -> Result<gtk::Expander> {
    let gtk_widget = gtk::Expander::new(None);

    match bargs.widget_use.children.len().cmp(&1) {
        Ordering::Less => {
            return Err(DiagError(gen_diagnostic!("expander must contain exactly one element", bargs.widget_use.span)).into());
        }
        Ordering::Greater => {
            let (_, additional_children) = bargs.widget_use.children.split_at(1);
            // we know that there is more than one child, so unwrapping on first and last here is fine.
            let first_span = additional_children.first().unwrap().span();
            let last_span = additional_children.last().unwrap().span();
            return Err(DiagError(gen_diagnostic!(
                "expander must contain exactly one element, but got more",
                first_span.to(last_span)
            ))
            .into());
        }
        Ordering::Equal => {
            let mut children = bargs.widget_use.children.iter().map(|child| {
                build_gtk_widget(
                    bargs.scope_graph,
                    bargs.widget_defs.clone(),
                    bargs.calling_scope,
                    child.clone(),
                    bargs.custom_widget_invocation.clone(),
                )
            });
            // we have exactly one child, we can unwrap
            let child = children.next().unwrap()?;
            gtk_widget.add(&child);
            child.show();
        }
    }

    def_widget!(bargs, _g, gtk_widget, {
        // @prop name - name of the expander
        prop(name: as_string) { gtk_widget.set_label(Some(&name)); },
        // @prop expanded - sets if the tree is expanded
        prop(expanded: as_bool) { gtk_widget.set_expanded(expanded); }
    });

    Ok(gtk_widget)
}

const WIDGET_NAME_REVEALER: &str = "revealer";
/// @widget revealer
/// @desc A widget that can reveal a child with an animation.
fn build_gtk_revealer(bargs: &mut BuilderArgs) -> Result<gtk::Revealer> {
    let gtk_widget = gtk::Revealer::new();
    def_widget!(bargs, _g, gtk_widget, {
        // @prop transition - the name of the transition. Possible values: $transition
        prop(transition: as_string = "crossfade") { gtk_widget.set_transition_type(parse_revealer_transition(&transition)?); },
        // @prop reveal - sets if the child is revealed or not
        prop(reveal: as_bool) { gtk_widget.set_reveal_child(reveal); },
        // @prop duration - the duration of the reveal transition. Default: "500ms"
        prop(duration: as_duration = Duration::from_millis(500)) { gtk_widget.set_transition_duration(duration.as_millis() as u32); },
    });
    Ok(gtk_widget)
}

const WIDGET_NAME_CHECKBOX: &str = "checkbox";
/// @widget a checkbox
/// @desc A checkbox that can trigger events on checked / unchecked.
fn build_gtk_checkbox(bargs: &mut BuilderArgs) -> Result<gtk::CheckButton> {
    let gtk_widget = gtk::CheckButton::new();
    def_widget!(bargs, _g, gtk_widget, {
        // @prop checked - whether the checkbox is toggled or not when created
        // @prop timeout - timeout of the command. Default: "200ms"
        // @prop onchecked - action (command) to be executed when checked by the user
        // @prop onunchecked - similar to onchecked but when the widget is unchecked
        prop(checked: as_bool = false, timeout: as_duration = Duration::from_millis(200), onchecked: as_string = "", onunchecked: as_string = "") {
            gtk_widget.set_active(checked);
            connect_signal_handler!(gtk_widget, gtk_widget.connect_toggled(move |gtk_widget| {
                run_command(timeout, if gtk_widget.is_active() { &onchecked } else { &onunchecked }, &[] as &[&str]);
            }));
       }
    });

    Ok(gtk_widget)
}

const WIDGET_NAME_COLOR_BUTTON: &str = "color-button";
/// @widget color-button
/// @desc A button opening a color chooser window
fn build_gtk_color_button(bargs: &mut BuilderArgs) -> Result<gtk::ColorButton> {
    let gtk_widget = gtk::ColorButton::builder().build();
    def_widget!(bargs, _g, gtk_widget, {
        // @prop use-alpha - bool to whether or not use alpha
        prop(use_alpha: as_bool) {gtk_widget.set_use_alpha(use_alpha);},

        // @prop onchange - runs the code when the color was selected
        // @prop timeout - timeout of the command. Default: "200ms"
        prop(timeout: as_duration = Duration::from_millis(200), onchange: as_string) {
            connect_signal_handler!(gtk_widget, gtk_widget.connect_color_set(move |gtk_widget| {
                run_command(timeout, &onchange, &[gtk_widget.rgba()]);
            }));
        }
    });

    Ok(gtk_widget)
}

const WIDGET_NAME_COLOR_CHOOSER: &str = "color-chooser";
/// @widget color-chooser
/// @desc A color chooser widget
fn build_gtk_color_chooser(bargs: &mut BuilderArgs) -> Result<gtk::ColorChooserWidget> {
    let gtk_widget = gtk::ColorChooserWidget::new();
    def_widget!(bargs, _g, gtk_widget, {
        // @prop use-alpha - bool to wether or not use alpha
        prop(use_alpha: as_bool) {gtk_widget.set_use_alpha(use_alpha);},

        // @prop onchange - runs the code when the color was selected
        // @prop timeout - timeout of the command. Default: "200ms"
        prop(timeout: as_duration = Duration::from_millis(200), onchange: as_string) {
            connect_signal_handler!(gtk_widget, gtk_widget.connect_color_activated(move |_a, color| {
                run_command(timeout, &onchange, &[*color]);
            }));
        }
    });

    Ok(gtk_widget)
}

const WIDGET_NAME_SCALE: &str = "scale";
/// @widget scale extends range, orientable
/// @desc A slider.
fn build_gtk_scale(bargs: &mut BuilderArgs) -> Result<gtk::Scale> {
    let gtk_widget = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&gtk::Adjustment::new(0.0, 0.0, 100.0, 1.0, 1.0, 1.0)));

    def_widget!(bargs, _g, gtk_widget, {
        // @prop flipped - flip the direction
        prop(flipped: as_bool) { gtk_widget.set_inverted(flipped) },

        // @prop marks - draw marks
        prop(marks: as_string) {
            gtk_widget.clear_marks();
            for mark in marks.split(',') {
                gtk_widget.add_mark(mark.trim().parse()?, gtk::PositionType::Bottom, None)
            }
        },

        // @prop draw-value - draw the value of the property
        prop(draw_value: as_bool = false) { gtk_widget.set_draw_value(draw_value) },

        // @prop round-digits - Sets the number of decimals to round the value to when it changes
        prop(round_digits: as_i32 = 0) { gtk_widget.set_round_digits(round_digits) }

    });
    Ok(gtk_widget)
}

const WIDGET_NAME_PROGRESS: &str = "progress";
/// @widget progress
/// @desc A progress bar. HINT: for the `width` property to work, you may need to set the `min-width` of `progressbar > trough` in your css.
fn build_gtk_progress(bargs: &mut BuilderArgs) -> Result<gtk::ProgressBar> {
    let gtk_widget = gtk::ProgressBar::new();
    def_widget!(bargs, _g, gtk_widget, {
        // @prop flipped - flip the direction
        prop(flipped: as_bool) { gtk_widget.set_inverted(flipped) },

        // @prop value - value of the progress bar (between 0-100)
        prop(value: as_f64) { gtk_widget.set_fraction(value / 100f64) },

        // @prop orientation - orientation of the progress bar. possible values: $orientation
        prop(orientation: as_string) { gtk_widget.set_orientation(parse_orientation(&orientation)?) },
    });

    Ok(gtk_widget)
}

const WIDGET_NAME_INPUT: &str = "input";
/// @widget input
/// @desc An input field. For this to be useful, set `focusable="true"` on the window.
fn build_gtk_input(bargs: &mut BuilderArgs) -> Result<gtk::Entry> {
    let gtk_widget = gtk::Entry::new();
    def_widget!(bargs, _g, gtk_widget, {
        // @prop value - the content of the text field
        prop(value: as_string) {
            gtk_widget.set_text(&value);
        },
        // @prop onchange - Command to run when the text changes. The placeholder `{}` will be replaced by the value
        // @prop timeout - timeout of the command. Default: "200ms"
        prop(timeout: as_duration = Duration::from_millis(200), onchange: as_string) {
            connect_signal_handler!(gtk_widget, gtk_widget.connect_changed(move |gtk_widget| {
                run_command(timeout, &onchange, &[gtk_widget.text().to_string()]);
            }));
        },
        // @prop onaccept - Command to run when the user hits return in the input field. The placeholder `{}` will be replaced by the value
        // @prop timeout - timeout of the command. Default: "200ms"
        prop(timeout: as_duration = Duration::from_millis(200), onaccept: as_string) {
            connect_signal_handler!(gtk_widget, gtk_widget.connect_activate(move |gtk_widget| {
                run_command(timeout, &onaccept, &[gtk_widget.text().to_string()]);
            }));
        },
        // @prop password - if the input is obscured
        prop(password: as_bool = false) {
            gtk_widget.set_visibility(!password);
        }
    });
    Ok(gtk_widget)
}

const WIDGET_NAME_BUTTON: &str = "button";
/// @widget button
/// @desc A button containing any widget as it's child. Events are triggered on release.
fn build_gtk_button(bargs: &mut BuilderArgs) -> Result<gtk::Button> {
    let gtk_widget = gtk::Button::new();

    def_widget!(bargs, _g, gtk_widget, {
        prop(
            // @prop timeout - timeout of the command. Default: "200ms"
            timeout: as_duration = Duration::from_millis(200),
            // @prop onclick - command to run when the button is activated either by leftclicking or keyboard
            onclick: as_string = "",
            // @prop onmiddleclick - command to run when the button is middleclicked
            onmiddleclick: as_string = "",
            // @prop onrightclick - command to run when the button is rightclicked
            onrightclick: as_string = ""
        ) {
            // animate button upon right-/middleclick (if gtk theme supports it)
            // since we do this, we can't use `connect_clicked` as that would always run `onclick` as well
            connect_signal_handler!(gtk_widget, gtk_widget.connect_button_press_event(move |button, _| {
                button.emit_activate();
                glib::Propagation::Proceed
            }));
            let onclick_ = onclick.clone();
            // mouse click events
            connect_signal_handler!(gtk_widget, gtk_widget.connect_button_release_event(move |_, evt| {
                match evt.button() {
                    1 => run_command(timeout, &onclick, &[] as &[&str]),
                    2 => run_command(timeout, &onmiddleclick, &[] as &[&str]),
                    3 => run_command(timeout, &onrightclick, &[] as &[&str]),
                    _ => {},
                }
                glib::Propagation::Proceed
            }));
            // keyboard events
            connect_signal_handler!(gtk_widget, gtk_widget.connect_key_release_event(move |_, evt| {
                match evt.scancode() {
                    // return
                    36 => run_command(timeout, &onclick_, &[] as &[&str]),
                    // space
                    65 => run_command(timeout, &onclick_, &[] as &[&str]),
                    _ => {},
                }
                glib::Propagation::Proceed
            }));
        }
    });
    Ok(gtk_widget)
}

/// @var icon-size - "menu", "small-toolbar", "toolbar", "large-toolbar", "button", "dnd", "dialog"
fn parse_icon_size(o: &str) -> Result<gtk::IconSize> {
    enum_parse! { "icon-size", o,
        "menu" => gtk::IconSize::Menu,
        "small-toolbar" | "toolbar" => gtk::IconSize::SmallToolbar,
        "large-toolbar" => gtk::IconSize::LargeToolbar,
        "button" => gtk::IconSize::Button,
        "dnd" => gtk::IconSize::Dnd,
        "dialog" => gtk::IconSize::Dialog,
    }
}

const WIDGET_NAME_IMAGE: &str = "image";
/// @widget image
/// @desc A widget displaying an image
fn build_gtk_image(bargs: &mut BuilderArgs) -> Result<gtk::Image> {
    let gtk_widget = gtk::Image::new();
    def_widget!(bargs, _g, gtk_widget, {
        // @prop path - path to the image file
        // @prop image-width - width of the image
        // @prop image-height - height of the image
        // @prop preserve-aspect-ratio - whether to keep the aspect ratio when resizing an image. Default: true, false doesn't work for all image types
        // @prop fill-svg - sets the color of svg images
        prop(path: as_string, image_width: as_i32 = -1, image_height: as_i32 = -1, preserve_aspect_ratio: as_bool = true, fill_svg: as_string = "") {
            if !path.ends_with(".svg") && !fill_svg.is_empty() {
                log::warn!("Fill attribute ignored, file is not an svg image");
            }

            if path.ends_with(".gif") {
                let pixbuf_animation = gtk::gdk_pixbuf::PixbufAnimation::from_file(std::path::PathBuf::from(path))?;
                gtk_widget.set_from_animation(&pixbuf_animation);
            } else {
                let pixbuf;
                // populate the pixel buffer
                if path.ends_with(".svg") && !fill_svg.is_empty() {
                    let svg_data = std::fs::read_to_string(std::path::PathBuf::from(path.clone()))?;
                    // The fastest way to add/change fill color
                    let svg_data = if svg_data.contains("fill=") {
                        let reg = regex::Regex::new(r#"fill="[^"]*""#)?;
                        reg.replace(&svg_data, &format!("fill=\"{}\"", fill_svg))
                    } else {
                        let reg = regex::Regex::new(r"<svg")?;
                        reg.replace(&svg_data, &format!("<svg fill=\"{}\"", fill_svg))
                    };
                    let stream = gtk::gio::MemoryInputStream::from_bytes(&gtk::glib::Bytes::from(svg_data.as_bytes()));
                    pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale(&stream, image_width, image_height, preserve_aspect_ratio, None::<&gtk::gio::Cancellable>)?;
                    stream.close(None::<&gtk::gio::Cancellable>)?;
                } else {
                    pixbuf = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(std::path::PathBuf::from(path), image_width, image_height, preserve_aspect_ratio)?;
                }
                gtk_widget.set_from_pixbuf(Some(&pixbuf));
            }
        },
        // @prop icon - name of a theme icon
        // @prop icon-size - size of the theme icon
        prop(icon: as_string, icon_size: as_string = "button") {
            gtk_widget.set_from_icon_name(Some(&icon), parse_icon_size(&icon_size)?);
        },
    });
    Ok(gtk_widget)
}

const WIDGET_NAME_BOX: &str = "box";
/// @widget box
/// @desc the main layout container
fn build_gtk_box(bargs: &mut BuilderArgs) -> Result<gtk::Box> {
    let gtk_widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    def_widget!(bargs, _g, gtk_widget, {
        // @prop spacing - spacing between elements
        prop(spacing: as_i32 = 0) { gtk_widget.set_spacing(spacing) },
        // @prop orientation - orientation of the box. possible values: $orientation
        prop(orientation: as_string) { gtk_widget.set_orientation(parse_orientation(&orientation)?) },
        // @prop space-evenly - space the widgets evenly.
        prop(space_evenly: as_bool = true) { gtk_widget.set_homogeneous(space_evenly) },
    });
    Ok(gtk_widget)
}

const WIDGET_NAME_OVERLAY: &str = "overlay";
/// @widget overlay
/// @desc a widget that places its children on top of each other. The overlay widget takes the size of its first child.
fn build_gtk_overlay(bargs: &mut BuilderArgs) -> Result<gtk::Overlay> {
    let gtk_widget = gtk::Overlay::new();

    // no def_widget because this widget has no props.

    match bargs.widget_use.children.len().cmp(&1) {
        Ordering::Less => {
            Err(DiagError(gen_diagnostic!("overlay must contain at least one element", bargs.widget_use.span)).into())
        }
        Ordering::Greater | Ordering::Equal => {
            let mut children = bargs.widget_use.children.iter().map(|child| {
                build_gtk_widget(
                    bargs.scope_graph,
                    bargs.widget_defs.clone(),
                    bargs.calling_scope,
                    child.clone(),
                    bargs.custom_widget_invocation.clone(),
                )
            });
            // we have more than one child, we can unwrap
            let first = children.next().unwrap()?;
            gtk_widget.add(&first);
            first.show();
            for child in children {
                let child = child?;
                gtk_widget.add_overlay(&child);
                gtk_widget.set_overlay_pass_through(&child, true);
                child.show();
            }
            Ok(gtk_widget)
        }
    }
}

const WIDGET_NAME_TOOLTIP: &str = "tooltip";
/// @widget tooltip
/// @desc A widget that have a custom tooltip. The first child is the content of the tooltip, the second one is the content of the widget.
fn build_tooltip(bargs: &mut BuilderArgs) -> Result<gtk::Box> {
    let gtk_widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    gtk_widget.set_has_tooltip(true);

    match bargs.widget_use.children.len().cmp(&2) {
        Ordering::Less => {
            Err(DiagError(gen_diagnostic!("tooltip must contain exactly 2 elements", bargs.widget_use.span)).into())
        }
        Ordering::Greater => {
            let (_, additional_children) = bargs.widget_use.children.split_at(2);
            // we know that there is more than two children, so unwrapping on first and last here is fine.
            let first_span = additional_children.first().unwrap().span();
            let last_span = additional_children.last().unwrap().span();
            Err(DiagError(gen_diagnostic!("tooltip must contain exactly 2 elements, but got more", first_span.to(last_span)))
                .into())
        }
        Ordering::Equal => {
            let mut children = bargs.widget_use.children.iter().map(|child| {
                build_gtk_widget(
                    bargs.scope_graph,
                    bargs.widget_defs.clone(),
                    bargs.calling_scope,
                    child.clone(),
                    bargs.custom_widget_invocation.clone(),
                )
            });
            // we know that we have exactly two children here, so we can unwrap here.
            let (tooltip, content) = children.next_tuple().unwrap();
            let (tooltip_content, content) = (tooltip?, content?);

            gtk_widget.add(&content);
            gtk_widget.connect_query_tooltip(move |_this, _x, _y, _keyboard_mode, tooltip| {
                tooltip.set_custom(Some(&tooltip_content));
                true
            });

            Ok(gtk_widget)
        }
    }
}

const WIDGET_NAME_CENTERBOX: &str = "centerbox";
/// @widget centerbox
/// @desc a box that must contain exactly three children, which will be layed out at the start, center and end of the container.
fn build_center_box(bargs: &mut BuilderArgs) -> Result<gtk::Box> {
    let gtk_widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    def_widget!(bargs, _g, gtk_widget, {
        // @prop orientation - orientation of the centerbox. possible values: $orientation
        prop(orientation: as_string) { gtk_widget.set_orientation(parse_orientation(&orientation)?) },
    });

    match bargs.widget_use.children.len().cmp(&3) {
        Ordering::Less => {
            Err(DiagError(gen_diagnostic!("centerbox must contain exactly 3 elements", bargs.widget_use.span)).into())
        }
        Ordering::Greater => {
            let (_, additional_children) = bargs.widget_use.children.split_at(3);
            // we know that there is more than three children, so unwrapping on first and left here is fine.
            let first_span = additional_children.first().unwrap().span();
            let last_span = additional_children.last().unwrap().span();
            Err(DiagError(gen_diagnostic!("centerbox must contain exactly 3 elements, but got more", first_span.to(last_span)))
                .into())
        }
        Ordering::Equal => {
            let mut children = bargs.widget_use.children.iter().map(|child| {
                build_gtk_widget(
                    bargs.scope_graph,
                    bargs.widget_defs.clone(),
                    bargs.calling_scope,
                    child.clone(),
                    bargs.custom_widget_invocation.clone(),
                )
            });
            // we know that we have exactly three children here, so we can unwrap here.
            let (first, center, end) = children.next_tuple().unwrap();
            let (first, center, end) = (first?, center?, end?);
            gtk_widget.pack_start(&first, true, true, 0);
            gtk_widget.set_center_widget(Some(&center));
            gtk_widget.pack_end(&end, true, true, 0);
            first.show();
            center.show();
            end.show();
            Ok(gtk_widget)
        }
    }
}

const WIDGET_NAME_MENU: &str = "menu";
fn build_gtk_menu(bargs: &mut BuilderArgs) -> Result<gtk::Box> {
    let gtk_widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let menu = gtk::Menu::new();

    let children = bargs.widget_use.children.iter().map(|child| {
        build_gtk_widget(
            bargs.scope_graph,
            bargs.widget_defs.clone(),
            bargs.calling_scope,
            child.clone(),
            bargs.custom_widget_invocation.clone(),
        )
    });

    for child in children {
        if let Some(menu_item) = child?.dynamic_cast_ref::<gtk::MenuItem>() {
            menu.append(menu_item);
        }
    }

    gtk_widget.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
    connect_signal_handler!(
        gtk_widget,
        gtk_widget.connect_button_press_event(move |_, evt| {
            match evt.button() {
                1 => menu.popdown(),
                3 => menu.popup_easy(evt.button(), evt.time()),
                _ => {}
            };

            glib::Propagation::Proceed
        })
    );

    Ok(gtk_widget)
}

const WIDGET_NAME_MENU_OPTION: &str = "option";
fn build_gtk_menu_option(bargs: &mut BuilderArgs) -> Result<gtk::MenuItem> {
    let gtk_widget = gtk::MenuItem::new();

    def_widget!(bargs, _g, gtk_widget, {
        prop(label: as_string) {
            gtk_widget.set_label(&label);
        },
        prop(onclick: as_string, timeout: as_duration = Duration::from_millis(200)) {
            gtk_widget.connect_activate(move |_| {
                run_command(timeout, &onclick, &[] as &[&str]);
            });
        }
    });

    Ok(gtk_widget)
}

const WIDGET_NAME_LISTBOX: &str = "listbox";
fn build_gtk_listbox(bargs: &mut BuilderArgs) -> Result<gtk::ListBox> {
    let gtk_widget = gtk::ListBox::new();

    connect_signal_handler!(
        gtk_widget,
        gtk_widget.connect_row_selected(|gtk_widget, listrow| {
            if let Some(row) = listrow {
                if let Some(adjustment) = gtk_widget.adjustment() {
                    let page_size = adjustment.page_size();
                    let height = row.allocation().height();
                    let width = row.allocation().width();
                    let x = row.allocation().x();
                    let y = row.allocation().y();

                    let offsety = page_size - height as f64;
                    let offsetx = page_size - width as f64;

                    if x >= 0 {
                        adjustment.set_value(x as f64 - offsetx / 2.0);
                    }

                    if y >= 0 {
                        adjustment.set_value(y as f64 - offsety / 2.0);
                    }
                }
                row.grab_focus();
            }
        })
    );

    def_widget!(bargs, _g, gtk_widget, {
        prop(position: as_i32 = 0) {
            gtk_widget.select_row(gtk_widget.row_at_index(position).as_ref());
        },
        prop(mode: as_string = "browse") {
            match mode.as_str() {
                "browse" => gtk_widget.set_selection_mode(gtk::SelectionMode::Browse),
                "multiple" => gtk_widget.set_selection_mode(gtk::SelectionMode::Multiple),
                "single" => gtk_widget.set_selection_mode(gtk::SelectionMode::Single),
                _ => gtk_widget.set_selection_mode(gtk::SelectionMode::None),
            }
        }
    });

    Ok(gtk_widget)
}

const WIDGET_NAME_LISTROW: &str = "listrow";
fn build_gtk_listrow(bargs: &mut BuilderArgs) -> Result<gtk::ListBoxRow> {
    let gtk_widget = gtk::ListBoxRow::new();

    def_widget!(bargs, _g, gtk_widget, {
        prop(activatable: as_bool = true, selectable: as_bool = true) {
            gtk_widget.set_activatable(activatable);
            gtk_widget.set_selectable(selectable);
        }
    });

    Ok(gtk_widget)
}

const WIDGET_NAME_SCROLL: &str = "scroll";
/// @widget scroll
/// @desc a container with a single child that can scroll.
fn build_gtk_scrolledwindow(bargs: &mut BuilderArgs) -> Result<gtk::ScrolledWindow> {
    // I don't have single idea of what those two generics are supposed to be, but this works.
    let gtk_widget = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);

    def_widget!(bargs, _g, gtk_widget, {
        // @prop hscroll - scroll horizontally
        // @prop vscroll - scroll vertically
        prop(hscroll: as_bool = true, vscroll: as_bool = true) {
            gtk_widget.set_policy(
                if hscroll { gtk::PolicyType::Automatic } else { gtk::PolicyType::Never },
                if vscroll { gtk::PolicyType::Automatic } else { gtk::PolicyType::Never },
            )
        },
    });

    Ok(gtk_widget)
}

const WIDGET_NAME_EVENTBOX: &str = "eventbox";
/// @widget eventbox
/// @desc a container which can receive events and must contain exactly one child. Supports `:hover` and `:active` css selectors.
fn build_gtk_event_box(bargs: &mut BuilderArgs) -> Result<gtk::EventBox> {
    let gtk_widget = gtk::EventBox::new();

    // Support :hover selector
    gtk_widget.connect_enter_notify_event(|gtk_widget, evt| {
        if evt.detail() != NotifyType::Inferior {
            gtk_widget.set_state_flags(gtk::StateFlags::PRELIGHT, false);
        }
        glib::Propagation::Proceed
    });

    gtk_widget.connect_leave_notify_event(|gtk_widget, evt| {
        if evt.detail() != NotifyType::Inferior {
            gtk_widget.unset_state_flags(gtk::StateFlags::PRELIGHT);
        }
        glib::Propagation::Proceed
    });

    // Support :active selector
    gtk_widget.connect_button_press_event(|gtk_widget, _| {
        gtk_widget.set_state_flags(gtk::StateFlags::ACTIVE, false);
        glib::Propagation::Proceed
    });

    gtk_widget.connect_button_release_event(|gtk_widget, _| {
        gtk_widget.unset_state_flags(gtk::StateFlags::ACTIVE);
        glib::Propagation::Proceed
    });

    gtk_widget.connect_key_press_event(|gtk_widget, _| {
        gtk_widget.set_state_flags(gtk::StateFlags::ACTIVE, false);
        glib::Propagation::Proceed
    });

    gtk_widget.connect_key_release_event(|gtk_widget, _| {
        gtk_widget.unset_state_flags(gtk::StateFlags::ACTIVE);
        glib::Propagation::Proceed
    });

    def_widget!(bargs, _g, gtk_widget, {
        // @prop timeout - timeout of the command. Default: "200ms"
        // @prop onscroll - event to execute when the user scrolls with the mouse over the widget. The placeholder `{}` used in the command will be replaced with either `up` or `down`.
        prop(timeout: as_duration = Duration::from_millis(200), onscroll: as_string) {
            gtk_widget.add_events(gdk::EventMask::SCROLL_MASK);
            gtk_widget.add_events(gdk::EventMask::SMOOTH_SCROLL_MASK);
            connect_signal_handler!(gtk_widget, gtk_widget.connect_scroll_event(move |_, evt| {
                let delta = evt.delta().1;
                if delta != 0f64 { // Ignore the first event https://bugzilla.gnome.org/show_bug.cgi?id=675959
                    run_command(timeout, &onscroll, &[if delta < 0f64 { "up" } else { "down" }]);
                }
                glib::Propagation::Proceed
            }));
        },
        // @prop timeout - timeout of the command. Default: "200ms"
        // @prop onhover - event to execute when the user hovers over the widget
        prop(timeout: as_duration = Duration::from_millis(200), onhover: as_string) {
            gtk_widget.add_events(gdk::EventMask::ENTER_NOTIFY_MASK);
            connect_signal_handler!(gtk_widget, gtk_widget.connect_enter_notify_event(move |_, evt| {
                if evt.detail() != NotifyType::Inferior {
                    run_command(timeout, &onhover, &[evt.position().0, evt.position().1]);
                }
                glib::Propagation::Proceed
            }));
        },
        // @prop timeout - timeout of the command. Default: "200ms"
        // @prop onhoverlost - event to execute when the user losts hovers over the widget
        prop(timeout: as_duration = Duration::from_millis(200), onhoverlost: as_string) {
            gtk_widget.add_events(gdk::EventMask::LEAVE_NOTIFY_MASK);
            connect_signal_handler!(gtk_widget, gtk_widget.connect_leave_notify_event(move |_, evt| {
                if evt.detail() != NotifyType::Inferior {
                    run_command(timeout, &onhoverlost, &[evt.position().0, evt.position().1]);
                }
                glib::Propagation::Proceed
            }));
        },
        // @prop cursor - Cursor to show while hovering (see [gtk3-cursors](https://docs.gtk.org/gdk3/ctor.Cursor.new_from_name.html) for possible names)
        prop(cursor: as_string) {
            gtk_widget.add_events(gdk::EventMask::ENTER_NOTIFY_MASK);
            gtk_widget.add_events(gdk::EventMask::LEAVE_NOTIFY_MASK);

            connect_signal_handler!(gtk_widget, gtk_widget.connect_enter_notify_event(move |widget, _evt| {
                if _evt.detail() != NotifyType::Inferior {
                    let display = gdk::Display::default();
                    let gdk_window = widget.window();
                    if let (Some(display), Some(gdk_window)) = (display, gdk_window) {
                        gdk_window.set_cursor(gdk::Cursor::from_name(&display, &cursor).as_ref());
                    }
                }
                glib::Propagation::Proceed
            }));
            connect_signal_handler!(gtk_widget, gtk_widget.connect_leave_notify_event(move |widget, _evt| {
                if _evt.detail() != NotifyType::Inferior {
                    let gdk_window = widget.window();
                    if let Some(gdk_window) = gdk_window {
                        gdk_window.set_cursor(None);
                    }
                }
                glib::Propagation::Proceed
            }));
        },
        // @prop timeout - timeout of the command. Default: "200ms"
        // @prop ondropped - Command to execute when something is dropped on top of this element. The placeholder `{}` used in the command will be replaced with the uri to the dropped thing.
        prop(timeout: as_duration = Duration::from_millis(200), ondropped: as_string) {
            gtk_widget.drag_dest_set(
                DestDefaults::ALL,
                &[
                    TargetEntry::new("text/uri-list", gtk::TargetFlags::OTHER_APP | gtk::TargetFlags::OTHER_WIDGET, 0),
                    TargetEntry::new("text/plain", gtk::TargetFlags::OTHER_APP | gtk::TargetFlags::OTHER_WIDGET, 0)
                ],
                gdk::DragAction::COPY,
            );
            connect_signal_handler!(gtk_widget, gtk_widget.connect_drag_data_received(move |_, _, _x, _y, selection_data, _target_type, _timestamp| {
                if let Some(data) = selection_data.uris().first(){
                    run_command(timeout, &ondropped, &[data.to_string(), "file".to_string()]);
                } else if let Some(data) = selection_data.text(){
                    run_command(timeout, &ondropped, &[data.to_string(), "text".to_string()]);
                }
            }));
        },

        // @prop dragvalue - URI that will be provided when dragging from this widget
        // @prop dragtype - Type of value that should be dragged from this widget. Possible values: $dragtype
        prop(dragvalue: as_string, dragtype: as_string = "file") {
            let dragtype = parse_dragtype(&dragtype)?;
            if dragvalue.is_empty() {
                gtk_widget.drag_source_unset();
            } else {
                let target_entry = match dragtype {
                    DragEntryType::File => TargetEntry::new("text/uri-list", gtk::TargetFlags::OTHER_APP | gtk::TargetFlags::OTHER_WIDGET, 0),
                    DragEntryType::Text => TargetEntry::new("text/plain", gtk::TargetFlags::OTHER_APP | gtk::TargetFlags::OTHER_WIDGET, 0),
                };
                gtk_widget.drag_source_set(
                    ModifierType::BUTTON1_MASK,
                    &[target_entry.clone()],
                    gdk::DragAction::COPY | gdk::DragAction::MOVE,
                );
                gtk_widget.drag_source_set_target_list(Some(&TargetList::new(&[target_entry])));
            }

            connect_signal_handler!(gtk_widget, if !dragvalue.is_empty(), gtk_widget.connect_drag_data_get(move |_, _, data, _, _| {
                match dragtype {
                    DragEntryType::File => data.set_uris(&[&dragvalue]),
                    DragEntryType::Text => data.set_text(&dragvalue),
                };
            }));
        },
        prop(
            // @prop timeout - timeout of the command. Default: "200ms"
            timeout: as_duration = Duration::from_millis(200),
            // @prop onclick - command to run when the widget is clicked
            onclick: as_string = "",
            // @prop onmiddleclick - command to run when the widget is middleclicked
            onmiddleclick: as_string = "",
            // @prop onrightclick - command to run when the widget is rightclicked
            onrightclick: as_string = ""
        ) {
            gtk_widget.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
            connect_signal_handler!(gtk_widget, gtk_widget.connect_button_release_event(move |_, evt| {
                match evt.button() {
                    1 => run_command(timeout, &onclick, &[] as &[&str]),
                    2 => run_command(timeout, &onmiddleclick, &[] as &[&str]),
                    3 => run_command(timeout, &onrightclick, &[] as &[&str]),
                    _ => {},
                }
                glib::Propagation::Proceed
            }));
        },
        prop(timeout: as_duration = Duration::from_millis(200), keypress: as_string) {
            gtk_widget.add_events(gdk::EventMask::KEY_PRESS_MASK);
            gtk_widget.add_events(gdk::EventMask::KEY_RELEASE_MASK);
            gtk_widget.add_events(gdk::EventMask::LEAVE_NOTIFY_MASK);

            let keyboard_handler = Rc::new(RefCell::new(KeyboardHandler::new()));
            let keyboard_handler1 = keyboard_handler.clone();
            let keyboard_handler2 = keyboard_handler.clone();
            let keypress1 = keypress.clone();

            connect_signal_handler!(gtk_widget, gtk_widget.connect_leave_notify_event(move |_, evt| {
                if evt.detail() != NotifyType::Inferior {
                    keyboard_handler2.borrow_mut().clear();
                }
                glib::Propagation::Proceed
            }));

            connect_signal_handler!(gtk_widget, gtk_widget.connect_key_press_event(move |_, evt| {
                if let Some(keys) = keyboard_handler1.borrow_mut().key_event(evt) {
                    run_command(timeout, &keypress1, &[keys]);
                }
                glib::Propagation::Proceed
            }));

            connect_signal_handler!(gtk_widget, gtk_widget.connect_key_release_event(move |_, _| {
                let mut kbd = keyboard_handler.borrow_mut();
                if let Some(keys) = kbd.get_keys() {
                    run_command(timeout, &keypress, &[keys]);
                }
                kbd.clear();
                glib::Propagation::Proceed
            }));
        }
    });

    Ok(gtk_widget)
}

#[derive(Debug)]
struct KeyboardHandler {
    pressed_keys: HashSet<(String, u32)>,
}

impl KeyboardHandler {
    fn new() -> KeyboardHandler {
        KeyboardHandler { pressed_keys: HashSet::new() }
    }

    fn clear(&mut self) {
        self.pressed_keys.clear();
    }

    fn key_event(&mut self, evt: &EventKey) -> Option<String> {
        let mut keychar = String::new();

        if let Some(unicode) = evt.keyval().to_unicode() {
            keychar.push(unicode);
        } else if let Some(keyname) = evt.keyval().name() {
            keychar.push_str(keyname.as_ref());
        }

        if self.pressed_keys.insert((keychar, evt.time())) {
            None
        } else {
            self.get_keys()
        }
    }

    fn get_keys(&mut self) -> Option<String> {
        let mut pressed_keys: Vec<(String, u32)> = self.pressed_keys.clone().into_iter().collect();
        pressed_keys.sort_by(|a, b| a.1.cmp(&b.1));
        let result: Vec<String> = pressed_keys
            .iter()
            .map(|key| {
                let keymapped = match key.0.as_str() {
                    "\t" => "tab",
                    " " => "space",
                    "\r" => "return",
                    "\u{7f}" => "delete",
                    "\u{1b}" => "escape",
                    "\u{8}" => "backspace",
                    "ISO_Level3_Shift" => "altgr",
                    "Alt_L" | "Alt_R" | "Meta_L" | "Meta_R" => "alt",
                    "Control_L" | "Control_R" => "ctrl",
                    "Shift_L" | "Shift_R" => "shift",
                    "Super_L" | "Super_R" => "super",
                    "Page_Down" => "pagedown",
                    "Page_Up" => "pageup",
                    key => key,
                };

                keymapped.to_lowercase()
            })
            .collect();

        if !result.is_empty() {
            Some(result.join("+"))
        } else {
            None
        }
    }
}

const WIDGET_NAME_DRAWINGAREA: &str = "drawingarea";

fn build_gtk_drawingarea(bargs: &mut BuilderArgs) -> Result<gtk::DrawingArea> {
    let gtk_widget = gtk::DrawingArea::new();

    fn ondraw(widget: gtk::DrawingArea, draw: String) -> glib::SignalHandlerId {
        let widget1 = widget.clone();
        widget.connect_draw(move |_, crx| {
            let crx0 = crx.clone();
            let crx1 = crx.clone();
            let crx2 = crx.clone();
            let crx3 = crx.clone();
            let crx4 = crx.clone();
            let crx5 = crx.clone();
            let crx6 = crx.clone();
            let crx7 = crx.clone();
            let crx8 = crx.clone();
            let crx9 = crx.clone();
            let crx10 = crx.clone();
            let crx11 = crx.clone();
            let crx12 = crx.clone();
            let mut engine = rhai::Engine::new();
            engine
                .register_fn("set_source_rgba", move |r: f64, g: f64, b: f64, a: f64| {
                    let r = if r > 1.0 { r / 255.0 } else { r };
                    let g = if g > 1.0 { g / 255.0 } else { g };
                    let b = if b > 1.0 { b / 255.0 } else { b };
                    crx0.set_source_rgba(r, g, b, a)
                })
                .register_fn("new_sub_path", move || {
                    crx1.new_sub_path();
                })
                .register_fn("arc", move |xc: f64, yc: f64, radius: f64, a1: f64, a2: f64| {
                    crx2.arc(xc, yc, radius, a1, a2);
                })
                .register_fn("arc_negative", move |xc: f64, yc: f64, radius: f64, a1: f64, a2: f64| {
                    crx3.arc_negative(xc, yc, radius, a1, a2);
                })
                .register_fn("close_path", move || {
                    crx4.close_path();
                })
                .register_fn("fill", move || {
                    let _ = crx5.fill();
                })
                .register_fn("set_line_width", move |width: f64| {
                    crx6.set_line_width(width);
                })
                .register_fn("set_fill_rule", move |rule: &str| match rule {
                    "winding" => crx7.set_fill_rule(cairo::FillRule::Winding),
                    "evenodd" => crx7.set_fill_rule(cairo::FillRule::EvenOdd),
                    rule => {
                        log::error!("Unreconized fill rule '{rule}'")
                    }
                })
                .register_fn("save", move || {
                    let _ = crx8.save();
                })
                .register_fn("restore", move || {
                    let _ = crx9.restore();
                })
                .register_fn("paint", move || {
                    let _ = crx10.paint();
                })
                .register_fn("set_source_linear", move |pat: cairo::LinearGradient| {
                    let _ = crx11.set_source(pat);
                })
                .register_fn("set_source_radial", move |pat: cairo::RadialGradient| {
                    let _ = crx12.set_source(pat);
                })
                .register_fn("LinearGradient", move |x0: f64, y0: f64, x1: f64, y1: f64| {
                    cairo::LinearGradient::new(x0, y0, x1, y1)
                })
                .register_fn(
                    "add_color_linear_rgba",
                    move |pat: cairo::LinearGradient, offset: f64, r: f64, g: f64, b: f64, a: f64| {
                        let r = if r > 1.0 { r / 255.0 } else { r };
                        let g = if g > 1.0 { g / 255.0 } else { g };
                        let b = if b > 1.0 { b / 255.0 } else { b };
                        pat.add_color_stop_rgba(offset, r, g, b, a);
                    },
                )
                .register_fn("RadialGradient", move |x0: f64, y0: f64, r0: f64, x1: f64, y1: f64, r1: f64| {
                    cairo::RadialGradient::new(x0, y0, r0, x1, y1, r1)
                })
                .register_fn(
                    "add_color_radial_rgba",
                    move |pat: cairo::RadialGradient, offset: f64, r: f64, g: f64, b: f64, a: f64| {
                        let r = if r > 1.0 { r / 255.0 } else { r };
                        let g = if g > 1.0 { g / 255.0 } else { g };
                        let b = if b > 1.0 { b / 255.0 } else { b };
                        pat.add_color_stop_rgba(offset, r, g, b, a);
                    },
                );
            let mut scope = rhai::Scope::new();
            scope.push_constant("PI", std::f64::consts::PI);
            scope.push_constant("WIDTH", widget1.allocation().width() as f64);
            scope.push_constant("HEIGHT", widget1.allocation().height() as f64);

            match engine.compile_with_scope(&scope, &draw) {
                Ok(ast) => match engine.run_ast(&ast) {
                    Ok(_) => {}
                    Err(err) => log::error!("{err}"),
                },
                Err(err) => log::error!("{err}"),
            }
            glib::Propagation::Proceed
        })
    }

    let gtk_widget1 = gtk_widget.clone();
    connect_signal_handler!(
        gtk_widget,
        gtk_widget.connect_realize(move |widget| {
            if let Some(clock) = widget.frame_clock() {
                let gtk_widget1 = gtk_widget1.clone();
                clock.connect_after_paint(move |_| {
                    gtk_widget1.queue_draw();
                });
            }
        })
    );

    def_widget!(bargs, _g, gtk_widget, {
        prop(draw: as_string) {
            connect_signal_handler!(gtk_widget, ondraw(gtk_widget.clone(), draw));
        }
    });
    Ok(gtk_widget)
}

const WIDGET_NAME_LABEL: &str = "label";
/// @widget label
/// @desc A text widget giving you more control over how the text is displayed
fn build_gtk_label(bargs: &mut BuilderArgs) -> Result<gtk::Label> {
    let gtk_widget = gtk::Label::new(None);

    def_widget!(bargs, _g, gtk_widget, {
        // @prop text - the text to display
        // @prop truncate - whether to truncate text (or pango markup). If `show-truncated` is `false`, or if `limit-width` has a value, this property has no effect and truncation is enabled.
        // @prop limit-width - maximum count of characters to display
        // @prop truncate-left - whether to truncate on the left side
        // @prop show-truncated - show whether the text was truncated. Disabling it will also disable dynamic truncation (the labels won't be truncated more than `limit-width`, even if there is not enough space for them), and will completly disable truncation on pango markup.
        // @prop unindent - whether to remove leading spaces
        prop(text: as_string, truncate: as_bool = false, limit_width: as_i32 = i32::MAX, truncate_left: as_bool = false, show_truncated: as_bool = true, unindent: as_bool = true) {
            let text = if show_truncated {
                // gtk does weird thing if we set max_width_chars to i32::MAX
                if limit_width == i32::MAX {
                    gtk_widget.set_max_width_chars(-1);
                } else {
                    gtk_widget.set_max_width_chars(limit_width);
                }
                if truncate || limit_width != i32::MAX {
                    if truncate_left {
                        gtk_widget.set_ellipsize(pango::EllipsizeMode::Start);
                    } else {
                        gtk_widget.set_ellipsize(pango::EllipsizeMode::End);
                    }
                } else {
                    gtk_widget.set_ellipsize(pango::EllipsizeMode::None);
                }

                text
            } else {
                gtk_widget.set_ellipsize(pango::EllipsizeMode::None);

                let limit_width = limit_width as usize;
                let char_count = text.chars().count();
                if char_count > limit_width {
                    if truncate_left {
                        text.chars().skip(char_count - limit_width).collect()
                    } else {
                        text.chars().take(limit_width).collect()
                    }
                } else {
                    text
                }
            };

            let text = unescape::unescape(&text).context(format!("Failed to unescape label text {}", &text))?;
            let text = if unindent { util::unindent(&text) } else { text };
            gtk_widget.set_text(&text);
        },
        // @prop markup - Pango markup to display
        // @prop truncate - whether to truncate text (or pango markup). If `show-truncated` is `false`, or if `limit-width` has a value, this property has no effect and truncation is enabled.
        // @prop limit-width - maximum count of characters to display
        // @prop truncate-left - whether to truncate on the left side
        // @prop show-truncated - show whether the text was truncatedd. Disabling it will also disable dynamic truncation (the labels won't be truncated more than `limit-width`, even if there is not enough space for them), and will completly disable truncation on pango markup.
        prop(markup: as_string, truncate: as_bool = false, limit_width: as_i32 = i32::MAX, truncate_left: as_bool = false, show_truncated: as_bool = true) {
            if (truncate || limit_width != i32::MAX) && show_truncated {
                // gtk does weird thing if we set max_width_chars to i32::MAX
                if limit_width == i32::MAX {
                    gtk_widget.set_max_width_chars(-1);
                } else {
                    gtk_widget.set_max_width_chars(limit_width);
                }

                if truncate_left {
                    gtk_widget.set_ellipsize(pango::EllipsizeMode::Start);
                } else {
                    gtk_widget.set_ellipsize(pango::EllipsizeMode::End);
                }
            } else {
                gtk_widget.set_ellipsize(pango::EllipsizeMode::None);
            }

            gtk_widget.set_markup(&markup);
        },
        // @prop wrap - Wrap the text. This mainly makes sense if you set the width of this widget.
        prop(wrap: as_bool) { gtk_widget.set_line_wrap(wrap) },
        // @prop angle - the angle of rotation for the label (between 0 - 360)
        prop(angle: as_f64 = 0) { gtk_widget.set_angle(angle) },
        // @prop gravity - the gravity of the string (south, east, west, north, auto). Text will want to face the direction of gravity.
        prop(gravity: as_string = "south") {
            gtk_widget.pango_context().set_base_gravity(parse_gravity(&gravity)?);
        },
        // @prop xalign - the alignment of the label text on the x axis (between 0 - 1, 0 -> left, 0.5 -> center, 1 -> right)
        prop(xalign: as_f64 = 0.5) { gtk_widget.set_xalign(xalign as f32) },
        // @prop yalign - the alignment of the label text on the y axis (between 0 - 1, 0 -> bottom, 0.5 -> center, 1 -> top)
        prop(yalign: as_f64 = 0.5) { gtk_widget.set_yalign(yalign as f32) },
        // @prop justify - the justification of the label text (left, right, center, fill)
        prop(justify: as_string = "left") {
            gtk_widget.set_justify(parse_justification(&justify)?);
        },
    });
    Ok(gtk_widget)
}

const WIDGET_NAME_LITERAL: &str = "literal";
/// @widget literal
/// @desc A widget that allows you to render arbitrary yuck.
fn build_gtk_literal(bargs: &mut BuilderArgs) -> Result<gtk::Box> {
    let gtk_widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
    gtk_widget.set_widget_name("literal");

    // TODO these clones here are dumdum
    let literal_use_span = bargs.widget_use.span;

    // the file id the literal-content has been stored under, for error reporting.
    let literal_file_id: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));

    let widget_defs = bargs.widget_defs.clone();
    let calling_scope = bargs.calling_scope;

    def_widget!(bargs, scope_graph, gtk_widget, {
        // @prop content - inline yuck that will be rendered as a widget.
        prop(content: as_string) {
            gtk_widget.children().iter().for_each(|w| gtk_widget.remove(w));
            if !content.is_empty() {
                let content_widget_use: DiagResult<_> = (||{
                    let ast = {
                        let mut yuck_files = error_handling_ctx::FILE_DATABASE.write().unwrap();
                        let (span, asts) = yuck_files.load_yuck_str("<literal-content>".to_string(), content)?;
                        if let Some(file_id) = literal_file_id.replace(Some(span.2)) {
                            yuck_files.unload(file_id);
                        }
                        yuck::parser::require_single_toplevel(span, asts)?
                    };

                    yuck::config::widget_use::WidgetUse::from_ast(ast)
                })();
                let content_widget_use = content_widget_use?;

                // TODO a literal should create a new scope, that I'm not even sure should inherit from root
                let child_widget = build_gtk_widget(scope_graph, widget_defs.clone(), calling_scope, content_widget_use, None)
                    .map_err(|e| {
                        let diagnostic = error_handling_ctx::anyhow_err_to_diagnostic(&e)
                            .unwrap_or_else(|| gen_diagnostic!(e))
                            .with_label(span_to_secondary_label(literal_use_span).with_message("Error in the literal used here"));
                        DiagError(diagnostic)
                    })?;
                gtk_widget.add(&child_widget);
                child_widget.show();
            }
        }
    });
    Ok(gtk_widget)
}

const WIDGET_NAME_CALENDAR: &str = "calendar";
/// @widget calendar
/// @desc A widget that displays a calendar
fn build_gtk_calendar(bargs: &mut BuilderArgs) -> Result<gtk::Calendar> {
    let gtk_widget = gtk::Calendar::new();
    def_widget!(bargs, _g, gtk_widget, {
        // @prop day - the selected day
        prop(day: as_f64) {
            if !(1f64..=31f64).contains(&day) {
                log::warn!("Calendar day is not a number between 1 and 31");
            } else {
                gtk_widget.set_day(day as i32)
            }
        },
        // @prop month - the selected month
        prop(month: as_f64) {
            if !(1f64..=12f64).contains(&month) {
                log::warn!("Calendar month is not a number between 1 and 12");
            } else {
                gtk_widget.set_month(month as i32 - 1)
            }
        },
        // @prop year - the selected year
        prop(year: as_f64) { gtk_widget.set_year(year as i32) },
        // @prop show-details - show details
        prop(show_details: as_bool) { gtk_widget.set_show_details(show_details) },
        // @prop show-heading - show heading line
        prop(show_heading: as_bool) { gtk_widget.set_show_heading(show_heading) },
        // @prop show-day-names - show names of days
        prop(show_day_names: as_bool) { gtk_widget.set_show_day_names(show_day_names) },
        // @prop show-week-numbers - show week numbers
        prop(show_week_numbers: as_bool) { gtk_widget.set_show_week_numbers(show_week_numbers) },
        // @prop onclick - command to run when the user selects a date. The `{0}` placeholder will be replaced by the selected day, `{1}` will be replaced by the month, and `{2}` by the year.
        // @prop timeout - timeout of the command. Default: "200ms"
        prop(timeout: as_duration = Duration::from_millis(200), onclick: as_string) {
            connect_signal_handler!(gtk_widget, gtk_widget.connect_day_selected(move |w| {
                run_command(
                    timeout,
                    &onclick,
                    &[w.day(), w.month(), w.year()]
                )
            }));
        }

    });

    Ok(gtk_widget)
}

const WIDGET_NAME_STACK: &str = "stack";
/// @widget stack
/// @desc A widget that displays one of its children at a time
fn build_gtk_stack(bargs: &mut BuilderArgs) -> Result<gtk::Stack> {
    let gtk_widget = gtk::Stack::new();

    if bargs.widget_use.children.is_empty() {
        return Err(DiagError(gen_diagnostic!("stack must contain at least one element", bargs.widget_use.span)).into());
    }

    let children = bargs.widget_use.children.iter().map(|child| {
        build_gtk_widget(
            bargs.scope_graph,
            bargs.widget_defs.clone(),
            bargs.calling_scope,
            child.clone(),
            bargs.custom_widget_invocation.clone(),
        )
    });

    for (i, child) in children.enumerate() {
        let child = child?;
        gtk_widget.add_named(&child, &i.to_string());
        child.show();
    }

    def_widget!(bargs, _g, gtk_widget, {
        // @prop selected - index of child which should be shown
        prop(selected: as_i32) { gtk_widget.set_visible_child_name(&selected.to_string()); },
        // @prop transition - the name of the transition. Possible values: $transition
        prop(transition: as_string = "crossfade") { gtk_widget.set_transition_type(parse_stack_transition(&transition)?); },
        // @prop same-size - sets whether all children should be the same size
        prop(same_size: as_bool = false) { gtk_widget.set_homogeneous(same_size); }
    });

    Ok(gtk_widget)
}

const WIDGET_NAME_TRANSFORM: &str = "transform";
/// @widget transform
/// @desc A widget that applies transformations to its content. They are applied in the following order: rotate -> translate -> scale
fn build_transform(bargs: &mut BuilderArgs) -> Result<Transform> {
    let w = Transform::new();
    def_widget!(bargs, _g, w, {
        // @prop rotate - the percentage to rotate
        prop(rotate: as_f64) { w.set_property("rotate", rotate); },
        // @prop transform-origin-x - x coordinate of origin of transformation (px or %)
        prop(transform_origin_x: as_string) { w.set_property("transform-origin-x", transform_origin_x) },
        // @prop transform-origin-y - y coordinate of origin of transformation (px or %)
        prop(transform_origin_y: as_string) { w.set_property("transform-origin-y", transform_origin_y) },
        // @prop translate-x - the amount to translate in the x direction (px or %)
        prop(translate_x: as_string) { w.set_property("translate-x", translate_x); },
        // @prop translate-y - the amount to translate in the y direction (px or %)
        prop(translate_y: as_string) { w.set_property("translate-y", translate_y); },
        // @prop scale-x - the amount to scale in the x direction (px or %)
        prop(scale_x: as_string) { w.set_property("scale-x", scale_x); },
        // @prop scale-y - the amount to scale in the y direction (px or %)
        prop(scale_y: as_string) { w.set_property("scale-y", scale_y); },
    });
    Ok(w)
}

const WIDGET_NAME_CIRCULAR_PROGRESS: &str = "circular-progress";
/// @widget circular-progress
/// @desc A widget that displays a circular progress bar
fn build_circular_progress_bar(bargs: &mut BuilderArgs) -> Result<CircProg> {
    let w = CircProg::new();
    def_widget!(bargs, _g, w, {
        // @prop value - the value, between 0 - 100
        prop(value: as_f64) { w.set_property("value", value.clamp(0.0, 100.0)); },
        // @prop start-at - the percentage that the circle should start at
        prop(start_at: as_f64) { w.set_property("start-at", start_at.clamp(0.0, 100.0)); },
        // @prop thickness - the thickness of the circle
        prop(thickness: as_f64) { w.set_property("thickness", thickness); },
        // @prop clockwise - wether the progress bar spins clockwise or counter clockwise
        prop(clockwise: as_bool) { w.set_property("clockwise", clockwise); },
        // @prop linecap - the progress bar shape style
        prop(linecap: as_string) { w.set_property("linecap", linecap) },
    });
    Ok(w)
}

const WIDGET_NAME_GRAPH: &str = "graph";
/// @widget graph
/// @desc A widget that displays a graph showing how a given value changes over time
fn build_graph(bargs: &mut BuilderArgs) -> Result<super::graph::Graph> {
    let w = super::graph::Graph::new();
    def_widget!(bargs, _g, w, {
        // @prop value - the value, between 0 - 100
        prop(value: as_f64) { w.set_property("value", value); },
        // @prop thickness - the thickness of the line
        prop(thickness: as_f64) { w.set_property("thickness", thickness); },
        // @prop time-range - the range of time to show
        prop(time_range: as_duration) { w.set_property("time-range", time_range.as_millis() as u64); },
        // @prop min - the minimum value to show (defaults to 0 if value_max is provided)
        // @prop max - the maximum value to show
        prop(min: as_f64 = 0, max: as_f64 = 100) {
            if min > max {
                return Err(DiagError(gen_diagnostic!(
                    format!("Graph's min ({min}) should never be higher than max ({max})")
                )).into());
            }
            w.set_property("min", min);
            w.set_property("max", max);
        },
        // @prop dynamic - whether the y range should dynamically change based on value
        prop(dynamic: as_bool) { w.set_property("dynamic", dynamic); },
        // @prop line-style - changes the look of the edges in the graph. Values: "miter" (default), "round",
        // "bevel"
        prop(line_style: as_string) { w.set_property("line-style", line_style); },
        // @prop flip-x - whether the x axis should go from high to low
        prop(flip_x: as_bool) { w.set_property("flip-x", flip_x); },
        // @prop flip-y - whether the y axis should go from high to low
        prop(flip_y: as_bool) { w.set_property("flip-y", flip_y); },
        // @prop vertical - if set to true, the x and y axes will be exchanged
        prop(vertical: as_bool) { w.set_property("vertical", vertical); },
    });
    Ok(w)
}

const WIDGET_NAME_SYSTRAY: &str = "systray";
/// @widget systray
/// @desc Tray for system notifier icons
fn build_systray(bargs: &mut BuilderArgs) -> Result<gtk::Box> {
    let gtk_widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let props = Rc::new(systray::Props::new());
    let props_clone = props.clone(); // copies for def_widget
    let props_clone2 = props.clone(); // copies for def_widget

    def_widget!(bargs, _g, gtk_widget, {
        // @prop spacing - spacing between elements
        prop(spacing: as_i32 = 0) { gtk_widget.set_spacing(spacing) },
        // @prop orientation - orientation of the box. possible values: $orientation
        prop(orientation: as_string) { gtk_widget.set_orientation(parse_orientation(&orientation)?) },
        // @prop space-evenly - space the widgets evenly.
        prop(space_evenly: as_bool = true) { gtk_widget.set_homogeneous(space_evenly) },
        // @prop icon-size - size of icons in the tray
        prop(icon_size: as_i32) {
            if icon_size <= 0 {
                log::warn!("Icon size is not a positive number");
            } else {
                props.icon_size(icon_size);
            }
        },
        // @prop prepend-new - prepend new icons.
        prop(prepend_new: as_bool = true) {
            *props_clone2.prepend_new.borrow_mut() = prepend_new;
        },
    });

    systray::spawn_systray(&gtk_widget, &props_clone);

    Ok(gtk_widget)
}

/// @var orientation - "vertical", "v", "horizontal", "h"
fn parse_orientation(o: &str) -> Result<gtk::Orientation> {
    enum_parse! { "orientation", o,
        "vertical" | "v" => gtk::Orientation::Vertical,
        "horizontal" | "h" => gtk::Orientation::Horizontal,
    }
}

enum DragEntryType {
    File,
    Text,
}

/// @var dragtype - "file", "text"
fn parse_dragtype(o: &str) -> Result<DragEntryType> {
    enum_parse! { "dragtype", o,
        "file" => DragEntryType::File,
        "text" => DragEntryType::Text,
    }
}

/// @var transition - "slideright", "slideleft", "slideup", "slidedown", "crossfade", "none"
fn parse_revealer_transition(t: &str) -> Result<gtk::RevealerTransitionType> {
    enum_parse! { "transition", t,
        "slideright" => gtk::RevealerTransitionType::SlideRight,
        "slideleft" => gtk::RevealerTransitionType::SlideLeft,
        "slideup" => gtk::RevealerTransitionType::SlideUp,
        "slidedown" => gtk::RevealerTransitionType::SlideDown,
        "fade" | "crossfade" => gtk::RevealerTransitionType::Crossfade,
        "none" => gtk::RevealerTransitionType::None,
    }
}

/// @var transition - "slideright", "slideleft", "slideup", "slidedown", "crossfade", "none"
fn parse_stack_transition(t: &str) -> Result<gtk::StackTransitionType> {
    enum_parse! { "transition", t,
        "slideright" => gtk::StackTransitionType::SlideRight,
        "slideleft" => gtk::StackTransitionType::SlideLeft,
        "slideup" => gtk::StackTransitionType::SlideUp,
        "slidedown" => gtk::StackTransitionType::SlideDown,
        "fade" | "crossfade" => gtk::StackTransitionType::Crossfade,
        "none" => gtk::StackTransitionType::None,
    }
}

/// @var alignment - "fill", "baseline", "center", "start", "end"
fn parse_align(o: &str) -> Result<gtk::Align> {
    enum_parse! { "alignment", o,
        "fill" => gtk::Align::Fill,
        "baseline" => gtk::Align::Baseline,
        "center" => gtk::Align::Center,
        "start" => gtk::Align::Start,
        "end" => gtk::Align::End,
    }
}

/// @var justification - "left", "right", "center", "fill"
fn parse_justification(j: &str) -> Result<gtk::Justification> {
    enum_parse! { "justification", j,
        "left" => gtk::Justification::Left,
        "right" => gtk::Justification::Right,
        "center" => gtk::Justification::Center,
        "fill" => gtk::Justification::Fill,
    }
}

/// @var gravity - "south", "east", "west", "north", "auto"
fn parse_gravity(g: &str) -> Result<gtk::pango::Gravity> {
    enum_parse! { "gravity", g,
        "south" => gtk::pango::Gravity::South,
        "east" => gtk::pango::Gravity::East,
        "west" => gtk::pango::Gravity::West,
        "north" => gtk::pango::Gravity::North,
        "auto" => gtk::pango::Gravity::Auto,
    }
}

/// Connect a function to the first map event of a widget. After that first map, the handler will get disconnected.
fn connect_first_map<W: IsA<gtk::Widget>, F: Fn(&W) + 'static>(widget: &W, func: F) {
    let signal_handler_id = std::rc::Rc::new(std::cell::RefCell::new(None));

    signal_handler_id.borrow_mut().replace(widget.connect_map({
        let signal_handler_id = signal_handler_id.clone();
        move |w| {
            if let Some(signal_handler_id) = signal_handler_id.borrow_mut().take() {
                w.disconnect(signal_handler_id);
            }
            func(w)
        }
    }));
}
