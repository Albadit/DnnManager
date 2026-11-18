//! DNN Manager's dialogs - each a GPUI view that emits [`dnnm_ui::dialog::DialogEvent`] with its result, drawn in
//! [`dnnm_ui::dialog::frame`] so they look alike. The app shows one at a time over the window and runs what it returns.
pub mod markdown;
pub mod guide;
pub mod tour;
pub mod whats_new;
pub mod help_menu;
pub mod deployment_export;
pub mod file_prompt;
pub mod database_check_list;
pub mod database_connection;
pub mod host_password;
pub mod rename_project;
pub mod edit_bindings;
pub mod edit_app_pool;
pub mod input;
pub mod setup_page;
pub mod upgrade_dnn;
pub mod dnn_upgrade_path;
pub mod password_input;
pub mod about;
pub mod iis_features;
pub mod setting_rules;
pub mod settings_cards;
pub mod settings_form;
pub mod settings_keyboard;
pub mod settings_page;
pub mod troubleshoot_page;
