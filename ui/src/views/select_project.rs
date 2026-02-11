use dioxus::prelude::*;
use std::rc::Rc;
use tracing::warn;

use lib::datum_cloud::OrganizationWithProjects;
use lib::SelectedContext;
use open::that;

use crate::{
    components::{
        select::{
            Select, SelectItemIndicator, SelectList, SelectOptionItem, SelectTrigger, SelectValue,
        },
        skeleton::Skeleton,
        Button, ButtonKind, IconSource,
    },
    state::AppState,
    Route,
};

#[component]
pub fn SelectProject() -> Element {
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    let state_for_load = state.clone();
    let orgs = use_signal(Vec::<OrganizationWithProjects>::new);
    let load_error = use_signal(|| None::<String>);
    let mut selected_org = use_signal(|| None::<String>);
    let mut selected_project = use_signal(|| None::<String>);
    let saving = use_signal(|| false);
    let save_error = use_signal(|| None::<String>);
    let refreshing = use_signal(|| false);

    use_future(move || {
        let state = state_for_load.clone();
        let mut orgs = orgs;
        let mut load_error = load_error;
        async move {
            match state.datum().orgs_and_projects().await {
                Ok(list) => {
                    orgs.set(list);
                    load_error.set(None);
                }
                Err(err) => {
                    load_error.set(Some(err.to_string()));
                }
            }
        }
    });

    let state_for_refresh = state.clone();
    let mut refresh_action = use_action(move |_: ()| {
        let state = state_for_refresh.clone();
        let mut orgs = orgs;
        let mut load_error = load_error;
        let mut refreshing = refreshing;
        async move {
            refreshing.set(true);
            match state.datum().orgs_and_projects().await {
                Ok(list) => {
                    orgs.set(list);
                    load_error.set(None);
                }
                Err(err) => {
                    load_error.set(Some(err.to_string()));
                }
            }
            refreshing.set(false);
            n0_error::Ok(())
        }
    });

    let state_for_selection = state.clone();
    use_effect(move || {
        let list = orgs.read().clone();
        if list.is_empty() || selected_org.read().is_some() {
            return;
        }

        if let Some(ctx) = state_for_selection.selected_context() {
            if let Some(org) = list.iter().find(|org| org.org.resource_id == ctx.org_id) {
                selected_org.set(Some(ctx.org_id.clone()));
                if org.projects.iter().any(|p| p.resource_id == ctx.project_id) {
                    selected_project.set(Some(ctx.project_id.clone()));
                    return;
                }
                if let Some(first_project) = org.projects.first() {
                    selected_project.set(Some(first_project.resource_id.clone()));
                    return;
                }
            }
        }

        let personal = list
            .iter()
            .find(|org| org.org.r#type.eq_ignore_ascii_case("personal"));
        let org = personal.or_else(|| list.first());
        if let Some(org) = org {
            selected_org.set(Some(org.org.resource_id.clone()));
            if let Some(first_project) = org.projects.first() {
                selected_project.set(Some(first_project.resource_id.clone()));
            }
        }
    });

    let save_and_nav = {
        let state = state.clone();
        Rc::new(move |org_id: String, project_id: String| {
            let orgs_snapshot = orgs.read().clone();
            let mut saving = saving;
            let mut save_error = save_error;
            saving.set(true);
            save_error.set(None);

            let org = match orgs_snapshot.iter().find(|o| o.org.resource_id == org_id) {
                Some(org) => org,
                None => {
                    save_error.set(Some("selected org not found".to_string()));
                    warn!("select: selected org not found");
                    saving.set(false);
                    return;
                }
            };
            let project = match org.projects.iter().find(|p| p.resource_id == project_id) {
                Some(project) => project,
                None => {
                    save_error.set(Some("selected project not found".to_string()));
                    warn!("select: selected project not found");
                    saving.set(false);
                    return;
                }
            };

            let ctx = SelectedContext {
                org_id,
                org_name: org.org.display_name.clone(),
                project_id,
                project_name: project.display_name.clone(),
            };

            spawn({
                let state = state.clone();
                let nav = nav;
                let mut saving = saving;
                let mut save_error = save_error;
                async move {
                    if let Err(err) = state.set_selected_context(Some(ctx)).await {
                        save_error.set(Some(err.to_string()));
                        warn!("select: failed to save selection: {err:#}");
                        saving.set(false);
                        return;
                    }
                    saving.set(false);
                    nav.push(Route::ProxiesList {});
                }
            });
        })
    };

    let content = if let Some(err) = load_error.read().clone() {
        rsx! {
            div { class: "rounded-lg border border-red-200 bg-red-50 p-4 text-alert-red",
                div { class: "text-sm font-semibold", "Failed to load your organizations and projects" }
                div { class: "text-sm mt-1 break-words", "{err}" }
            }
        }
    } else if orgs.read().is_empty() {
        rsx! {
            div { class: "flex flex-col gap-4 w-full",
                div { class: "flex flex-col gap-2",
                    Skeleton { class: "h-4 w-24" }
                    Skeleton { class: "h-9 w-full" }
                }
                div { class: "flex flex-col gap-2",
                    Skeleton { class: "h-4 w-20" }
                    Skeleton { class: "h-9 w-full" }
                }
            }
        }
    } else {
        let selected_org_id = selected_org.read().clone();
        let selected_project_id = selected_project.read().clone();
        let orgs_snapshot = orgs.read().clone();
        let org_options: Vec<(String, String)> = orgs_snapshot
            .iter()
            .map(|org| (org.org.resource_id.clone(), org.org.display_name.clone()))
            .collect();
        let project_options: Vec<(String, String)> = selected_org_id
            .as_ref()
            .and_then(|org_id| {
                orgs_snapshot
                    .iter()
                    .find(|org| &org.org.resource_id == org_id)
            })
            .map(|org| {
                org.projects
                    .iter()
                    .map(|p| (p.resource_id.clone(), p.display_name.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let project_disabled = selected_org_id.is_none();
        let project_placeholder = if selected_org_id.is_none() {
            "Select an organization first".to_string()
        } else {
            "Select a project".to_string()
        };
        let has_no_projects = project_options.is_empty() && selected_org_id.is_some();
        let create_project_url = if has_no_projects {
            let org_slug = selected_org_id.clone().unwrap_or_default();
            format!("{}/org/{org_slug}/projects", state.datum().web_url())
        } else {
            String::new()
        };
        rsx! {
            div { class: "space-y-4",
                div { class: "flex flex-col gap-2",
                    label { class: "text-xs text-form-label/90", "Organization" }
                    Select {
                        value: selected_org_id.clone(),
                        on_value_change: move |value: Option<String>| {
                            let Some(value) = value else { return };
                            let orgs_snapshot = orgs.read().clone();
                            let org = orgs_snapshot.iter().find(|o| o.org.resource_id == value);
                            selected_org.set(Some(value.clone()));
                            if let Some(org) = org {
                                let first = org.projects.first().map(|p| p.resource_id.clone());
                                selected_project.set(first);
                            } else {
                                selected_project.set(None);
                            }
                        },
                        placeholder: "Select an organization".to_string(),
                        disabled: false,
                        SelectTrigger { SelectValue {} }
                        SelectList {
                            if org_options.is_empty() {
                                SelectOptionItem {
                                    value: "".to_string(),
                                    text_value: "No results".to_string(),
                                    index: 0,
                                    disabled: true,
                                    "No results"
                                }
                            } else {
                                for (i , (id , label)) in org_options.clone().into_iter().enumerate() {
                                    SelectOptionItem {
                                        value: id.clone(),
                                        text_value: label.clone(),
                                        index: i,
                                        div { class: "flex w-full justify-between items-center",
                                            span { class: "truncate", "{label}" }
                                            div { class: "text-1xs text-foreground/50 font-mono",
                                                "{id}"
                                            }
                                        }
                                        SelectItemIndicator {}
                                    }
                                }
                            }
                        }
                    }
                }
                if has_no_projects {
                    // Show link to create project when org is selected but has no projects
                    div { class: "flex flex-col gap-2",
                        label { class: "text-xs text-form-label/90", "Project" }
                        div { class: "rounded-md border border-app-border bg-content-background p-4",
                            div { class: "text-sm text-foreground mb-3",
                                "No projects found in this organization."
                            }
                            div { class: "flex gap-2",
                                Button {
                                    text: "Create a project".to_string(),
                                    kind: ButtonKind::Primary,
                                    onclick: move |_| {
                                        let _ = that(&create_project_url);
                                    },
                                    trailing_icon: Some(IconSource::Named("external-link".into())),
                                }
                                Button {
                                    text: "Refresh".to_string(),
                                    kind: ButtonKind::Outline,
                                    class: if refreshing() { Some("opacity-60 pointer-events-none".to_string()) } else { None },
                                    onclick: move |_| {
                                        refresh_action.call(());
                                    },
                                    trailing_icon: if refreshing() { Some(IconSource::Named("loader-circle".into())) } else { None },
                                }
                            }
                        }
                    }
                } else {
                    div { class: "flex flex-col gap-2",
                        label { class: "text-xs text-form-label/80", "Project" }
                        Select {
                            value: selected_project_id.clone(),
                            on_value_change: move |value: Option<String>| {
                                let Some(value) = value else { return };
                                selected_project.set(Some(value));
                            },
                            placeholder: project_placeholder.clone(),
                            disabled: project_disabled,
                            SelectTrigger { SelectValue {} }
                            SelectList {
                                if project_options.is_empty() {
                                    SelectOptionItem {
                                        value: "".to_string(),
                                        text_value: "No results".to_string(),
                                        index: 0,
                                        disabled: true,
                                        "No results"
                                    }
                                } else {
                                    for (i , (id , label)) in project_options.clone().into_iter().enumerate() {
                                        SelectOptionItem {
                                            value: id.clone(),
                                            text_value: label.clone(),
                                            index: i,
                                            div { class: "flex w-full justify-between items-center",
                                                span { class: "truncate", "{label}" }
                                                div { class: "text-1xs text-foreground/50 font-mono",
                                                    "{id}"
                                                }
                                            }
                                            SelectItemIndicator {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    rsx! {
        div { class: "h-full overflow-hidden flex flex-col bg-content-background text-foreground",
            // Content row with main content
            div { class: "flex flex-1 min-h-0 relative",
                // Main content area with skeleton tunnel cards
                div { class: "flex-1 min-h-0 overflow-y-auto bg-content-background",
                    div { class: "max-w-4xl mx-auto space-y-5",
                        // Tunnel card skeletons
                        for _ in 0..3 {
                            div { class: "bg-foreground/10 rounded-lg border border-app-border shadow-card h-48" }
                        }
                    }
                }
            }
            // Overlay (full screen backdrop)
            div { class: "bg-foreground/30 mt-[32px] fixed inset-0 z-50 flex items-center justify-center animate-in fade-in duration-100",
                // Form dialog centered on top
                div { class: "w-full max-w-lg mx-auto p-8 bg-card-background rounded-lg border border-card-border shadow-card relative z-50",
                    div { class: "mb-6",
                        h1 { class: "text-xl font-medium text-foreground",
                            "Where to manage your tunnels"
                        }
                    }
                    {content}
                    div { class: "mt-6 flex justify-start",
                        Button {
                            text: "Continue".to_string(),
                            class: if saving() { Some("opacity-60 pointer-events-none".to_string()) } else if selected_org.read().is_some() && selected_project.read().is_some() { None } else { Some("opacity-50 cursor-not-allowed".to_string()) },
                            onclick: move |_| {
                                let org = selected_org.read().clone().unwrap_or_default();
                                let project = selected_project.read().clone().unwrap_or_default();
                                if org.is_empty() || project.is_empty() {
                                    return;
                                }
                                let save_and_nav = save_and_nav.clone();
                                save_and_nav(org, project);
                            },
                        }
                        if saving() {
                            div { class: "text-sm text-slate-500 ml-3", "Saving selection…" }
                        }
                    }
                    if let Some(err) = save_error.read().clone() {
                        div { class: "mt-4 rounded-xl border border-red-200 bg-red-50 p-4 text-alert-red",
                            div { class: "text-sm font-semibold", "Failed to save selection" }
                            div { class: "text-sm mt-1 break-words", "{err}" }
                        }
                    }
                }
            }
        }
    }
}
