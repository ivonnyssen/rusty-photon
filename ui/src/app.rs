use dioxus::prelude::*;
use dioxus_free_icons::{icons::bs_icons::*, Icon};

#[component]
fn Toolbar() -> Element {
    rsx! {
        div { 
            class: "toolbar",
            button { class: "tool-button", aria_label: "Open", Icon { icon: BsFolder } }
            button { class: "tool-button", aria_label: "Save", Icon { icon: BsSave } }
            span { class: "separator" }
            button { class: "tool-button", aria_label: "Play", Icon { icon: BsPlayFill } }
            button { class: "tool-button", aria_label: "Pause", Icon { icon: BsPauseFill } }
            button { class: "tool-button", aria_label: "Stop", Icon { icon: BsStopFill } }
        }
    }
}

#[derive(Props, PartialEq, Clone)]
struct SidePanelProps {
    title: String,
}

#[component]
fn SidePanel(props: SidePanelProps) -> Element {
    rsx! {
        div { 
            class: "side-panel",
            div { 
                class: "panel-header",
                h3 { "{props.title}" }
            }
            div { 
                class: "panel-content",
                // Add panel content here
            }
        }
    }
}

#[component]
pub fn App() -> Element {
    rsx! {
        div { 
            class: "app-container",
            // Top toolbar
            Toolbar {}
            
            div { 
                class: "main-content",
                // Left sidebar
                div { 
                    class: "left-sidebar",
                    SidePanel { title: "Sequence Settings".to_string() }
                    SidePanel { title: "Image Statistics".to_string() }
                }
                
                // Main image area
                div { 
                    class: "image-area",
                    div { 
                        class: "image-container",
                        img { 
                            src: "https://raw.githubusercontent.com/ivonnyssen/rusty-photon/main/assets/logo.png",
                            alt: "Preview"
                        }
                    }
                }
                
                // Right sidebar
                div { 
                    class: "right-sidebar",
                    SidePanel { title: "Summary".to_string() }
                    SidePanel { title: "Focus".to_string() }
                }
            }
        }
    }
}
