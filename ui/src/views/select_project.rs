use dioxus::prelude::*;
use std::rc::Rc;
use tracing::warn;

use lib::datum_cloud::OrganizationWithProjects;
use lib::SelectedContext;

use crate::{
    components::{Button, SelectDropdown, SelectItem},
    state::AppState,
    Route,
};

#[component]
pub fn SelectProject() -> Element {
    let nav = use_navigator();
    let state = consume_context::<AppState>();
    let state_for_load = state.clone();
    let mut orgs = use_signal(Vec::<OrganizationWithProjects>::new);
    let mut load_error = use_signal(|| None::<String>);
    let mut selected_org = use_signal(|| None::<String>);
    let mut selected_project = use_signal(|| None::<String>);
    let mut auto_saved = use_signal(|| false);
    let saving = use_signal(|| false);
    let save_error = use_signal(|| None::<String>);

    use_future(move || {
        let state = state_for_load.clone();
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
        let nav = nav.clone();
        let orgs = orgs.clone();
        let saving = saving.clone();
        let save_error = save_error.clone();
        Rc::new(move |org_id: String, project_id: String| {
            let orgs_snapshot = orgs.read().clone();
            let mut saving = saving.clone();
            let mut save_error = save_error.clone();
            saving.set(true);
            save_error.set(None);

            let org = match orgs_snapshot
                .iter()
                .find(|o| o.org.resource_id == org_id)
            {
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
                let nav = nav.clone();
                let mut saving = saving.clone();
                let mut save_error = save_error.clone();
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

    let save_and_nav_for_auto = save_and_nav.clone();
    use_effect(move || {
        if *auto_saved.read() {
            return;
        }
        let selected_org_id = selected_org.read().clone();
        let selected_project_id = selected_project.read().clone();
        let list = orgs.read().clone();
        if selected_org_id.is_none() {
            return;
        }
        let org_id = selected_org_id.clone().unwrap_or_default();
        let org = list.iter().find(|org| org.org.resource_id == org_id);
        if let Some(org) = org {
            if org.projects.len() == 1 && selected_project_id.is_some() {
                auto_saved.set(true);
                if let Some(project_id) = selected_project_id.clone() {
                    let save_and_nav = save_and_nav_for_auto.clone();
                    save_and_nav(selected_org_id.clone().unwrap_or_default(), project_id);
                }
            }
        }
    });

    let content = if let Some(err) = load_error.read().clone() {
        rsx! {
            div { class: "rounded-xl border border-red-200 bg-red-50 p-4 text-red-800",
                div { class: "text-sm font-semibold", "Failed to load orgs/projects" }
                div { class: "text-sm mt-1 break-words", "{err}" }
            }
        }
    } else if orgs.read().is_empty() {
        rsx! {
            div { class: "rounded-xl border border-[#e3e7ee] bg-white/70 p-6 text-center",
                div { class: "text-base font-semibold text-slate-900", "Loading organizations…" }
                div { class: "text-sm text-slate-600 mt-2", "Fetching your orgs and projects." }
            }
        }
    } else {
        let selected_org_id = selected_org.read().clone();
        let selected_project_id = selected_project.read().clone();
        let org_items: Vec<OrganizationWithProjects> = orgs.read().iter().cloned().collect();
        let selected_org_snapshot = selected_org_id.as_ref().and_then(|org_id| {
            orgs.read()
                .iter()
                .find(|org| &org.org.resource_id == org_id)
                .cloned()
        });
        let project_items = selected_org_snapshot.as_ref().map(|org| org.projects.clone());
        let project_disabled = selected_org_id.is_none();
        rsx! {
            div { class: "space-y-8",
                SelectDropdown {
                    label: "Organization".to_string(),
                    placeholder: "Select an organization".to_string(),
                    items: org_items
                        .iter()
                        .map(|org| SelectItem {
                            id: org.org.resource_id.clone(),
                            label: org.org.display_name.clone(),
                            subtitle: Some(org.org.resource_id.clone()),
                        })
                        .collect(),
                    selected: selected_org_id.clone(),
                    on_select: move |value: String| {
                        let orgs_snapshot = orgs.read().clone();
                        let org = orgs_snapshot.iter().find(|org| org.org.resource_id == value);
                        selected_org.set(Some(value.clone()));
                        if let Some(org) = org {
                            let first_project = org.projects.first().map(|p| p.resource_id.clone());
                            selected_project.set(first_project);
                        } else {
                            selected_project.set(None);
                        }
                    },
                    searchable: true,
                    search_placeholder: "Search organizations…".to_string(),
                }
                div { class: "h-6" }
                SelectDropdown {
                    label: "Project".to_string(),
                    placeholder: if selected_org_id.is_none() {
                        "Select an organization first".to_string()
                    } else {
                        "Select a project".to_string()
                    },
                    items: project_items
                        .unwrap_or_default()
                        .iter()
                        .map(|project| SelectItem {
                            id: project.resource_id.clone(),
                            label: project.display_name.clone(),
                            subtitle: Some(project.resource_id.clone()),
                        })
                        .collect(),
                    selected: selected_project_id.clone(),
                    on_select: move |value: String| {
                        selected_project.set(Some(value));
                    },
                    disabled: project_disabled,
                    searchable: true,
                    search_placeholder: "Search projects…".to_string(),
                }
            }
        }
    };

    rsx! {
        div { class: "w-full grid h-screen bg-[#f4f4f1] place-items-center",
            div { class: "w-full max-w-xl mx-auto p-8 bg-white rounded-2xl border border-[#e3e7ee] shadow-[0_14px_34px_rgba(17,24,39,0.12)]",
                div { class: "mb-6",
                    h1 { class: "text-2xl font-semibold text-slate-900", "Select your org & project" }
                    p { class: "text-sm text-slate-600 mt-2",
                        "Choose where to manage tunnels. You can change this later from the header."
                    }
                }
                {content}
                div { class: "mt-8 flex justify-end",
                    Button {
                        text: "Continue".to_string(),
                        class: if saving() {
                            Some("opacity-60 pointer-events-none".to_string())
                        } else if selected_org.read().is_some() && selected_project.read().is_some() {
                            None
                        } else {
                            Some("opacity-50 cursor-not-allowed".to_string())
                        },
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
                    div { class: "mt-4 rounded-xl border border-red-200 bg-red-50 p-4 text-red-800",
                        div { class: "text-sm font-semibold", "Failed to save selection" }
                        div { class: "text-sm mt-1 break-words", "{err}" }
                    }
                }
            }
        }
    }
}
