use anyhow::Result;
use chrono::DateTime;
use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, ValueEnum, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ProfileType {
    Dev,       // Development repository
    Installed, // Installed application
    Binary,    // Binary/executable location
    Config,    // Configuration files
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AppProfile {
    profile_type: ProfileType,
    location: PathBuf,
    machine_name: Option<String>,
    notes: Option<String>,
    active: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct App {
    name: String,
    #[serde(default)]
    profiles: Vec<AppProfile>,
    // Legacy field for migration
    directory: Option<PathBuf>,
    tags: Vec<String>,
    github_repo: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct AppsData {
    apps: HashMap<String, App>,
}

#[derive(Parser)]
#[command(name = "apps-helper")]
#[command(about = "A CLI tool to manage your app usage and development")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    App {
        #[arg(long, help = "Get specific app (supports fuzzy matching)")]
        get: Option<String>,
        #[command(subcommand)]
        subcommand: Option<AppCommands>,
    },
}

#[derive(Subcommand)]
enum AppCommands {
    Add {
        name: Option<String>,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, help = "Use current directory as app directory and derive name from directory name")]
        current_dir: bool,
    },
    List,
    Get,
    Remove {
        #[arg(long, help = "Remove app by name (supports fuzzy matching)")]
        get: Option<String>,
        #[arg(long, help = "Remove app that matches current directory")]
        current_dir: bool,
    },
    Profile {
        #[command(subcommand)]
        profile_command: ProfileCommands,
    },
}

#[derive(Subcommand)]
enum ProfileCommands {
    Add {
        #[arg(long, value_enum)]
        r#type: ProfileType,
        #[arg(long)]
        location: Option<PathBuf>,
        #[arg(long, help = "Use current directory as profile location")]
        current_dir: bool,
        #[arg(long)]
        machine: Option<String>,
        #[arg(long)]
        notes: Option<String>,
    },
    List,
    Activate {
        #[arg(long, value_enum)]
        r#type: ProfileType,
    },
    Remove {
        #[arg(long, value_enum)]
        r#type: ProfileType,
    },
}

fn get_data_file_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME environment variable not set");
    PathBuf::from(home).join(".apps-helper").join("apps.json")
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::App { get, subcommand } => {
            handle_app_command(get, subcommand)?;
        }
    }

    Ok(())
}

fn handle_app_command(get_app: Option<String>, subcommand: Option<AppCommands>) -> Result<()> {
    match subcommand {
        Some(AppCommands::Add { name, dir, tags, current_dir }) => {
            add_app(&name, &dir, &tags, current_dir)?;
        }
        Some(AppCommands::List) => {
            list_apps()?;
        }
        Some(AppCommands::Get) => {
            if let Some(app_name) = get_app {
                get_app_info(&app_name)?;
            } else {
                return Err(anyhow::anyhow!("--get is required for the get command"));
            }
        }
        Some(AppCommands::Remove { get, current_dir }) => {
            let search_term = get.or(get_app);
            remove_app(&search_term, current_dir)?;
        }
        Some(AppCommands::Profile { profile_command }) => {
            if let Some(app_name) = get_app {
                handle_profile_command(&app_name, profile_command)?;
            } else {
                return Err(anyhow::anyhow!("--get is required for profile commands"));
            }
        }
        None => {
            // No subcommand provided
            if let Some(app_name) = get_app {
                // --get provided without subcommand, show app info
                get_app_info(&app_name)?;
            } else {
                return Err(anyhow::anyhow!("Please provide either --get <app-name> or a subcommand (add, list, remove, profile)"));
            }
        }
    }
    Ok(())
}

fn handle_profile_command(app_name: &str, command: ProfileCommands) -> Result<()> {
    let mut data = load_data()?;
    
    let app = find_app_by_name_mut(&mut data, app_name);
    
    match app {
        Some(app) => {
            match command {
                ProfileCommands::Add { r#type, location, current_dir, machine, notes } => {
                    let profile_location = if current_dir {
                        std::env::current_dir()?
                    } else {
                        location.ok_or_else(|| anyhow::anyhow!("Either --location or --current-dir must be specified"))?
                    };
                    
                    // Use provided machine name or default to current machine
                    let machine_name = machine.or_else(get_machine_name);
                    
                    let app_name = app.name.clone();
                    add_profile(app, r#type, profile_location, machine_name, notes)?;
                    let _ = app; // Release the mutable borrow
                    save_data(&data)?;
                    println!("Added profile to app: {}", app_name);
                }
                ProfileCommands::List => {
                    list_profiles(app);
                }
                ProfileCommands::Activate { r#type } => {
                    let app_name = app.name.clone();
                    activate_profile(app, r#type)?;
                    let _ = app; // Release the mutable borrow
                    save_data(&data)?;
                    println!("Activated {:?} profile for app: {}", r#type, app_name);
                }
                ProfileCommands::Remove { r#type } => {
                    let app_name = app.name.clone();
                    remove_profile(app, r#type)?;
                    let _ = app; // Release the mutable borrow
                    save_data(&data)?;
                    println!("Removed {:?} profile from app: {}", r#type, app_name);
                }
            }
        }
        None => {
            println!("App '{}' not found.", app_name);
        }
    }
    
    Ok(())
}

fn add_app(name: &Option<String>, dir: &Option<PathBuf>, tags: &Option<String>, use_current_dir: bool) -> Result<()> {
    let mut data = load_data()?;
    
    let tag_list = tags
        .as_ref()
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_else(Vec::new);

    let directory = if use_current_dir {
        Some(std::env::current_dir()?)
    } else {
        dir.clone()
    };

    let app_name = match name {
        Some(n) => n.clone(),
        None => {
            if use_current_dir {
                let current_dir = std::env::current_dir()?;
                current_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string()
            } else {
                return Err(anyhow::anyhow!("App name is required when not using --current-dir"));
            }
        }
    };

    // Check if app already exists
    if data.apps.contains_key(&app_name) {
        return Err(anyhow::anyhow!("App '{}' already exists. Use a different name or remove the existing app first.", app_name));
    }

    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    
    // Get machine name
    let machine_name = get_machine_name();
    
    // Create initial profile if directory is specified
    let mut profiles = Vec::new();
    if let Some(dir) = &directory {
        profiles.push(AppProfile {
            profile_type: ProfileType::Dev,
            location: dir.clone(),
            machine_name: machine_name.clone(),
            notes: None,
            active: true,
        });
    }
    
    let app = App {
        name: app_name.clone(),
        profiles: profiles.clone(),
        directory: directory.clone(), // Keep for legacy compatibility
        tags: tag_list.clone(),
        github_repo: None,
        created_at: now.clone(),
        updated_at: now,
    };

    // Show preview of what will be added
    println!("Adding app:");
    println!("  Name: {}", app_name);
    if !profiles.is_empty() {
        println!("  Profiles:");
        for profile in &profiles {
            println!("    {:?}: {}", profile.profile_type, profile.location.display());
            if let Some(ref machine) = profile.machine_name {
                println!("      Machine: {}", machine);
            }
        }
    }
    if !tag_list.is_empty() {
        println!("  Tags: {}", tag_list.join(", "));
    }
    println!();

    data.apps.insert(app_name.clone(), app);
    save_data(&data)?;
    
    println!("✓ Added app: {}", app_name);
    Ok(())
}

fn list_apps() -> Result<()> {
    let data = load_data()?;
    
    if data.apps.is_empty() {
        println!("No apps found.");
        return Ok(());
    }
    
    println!("Apps:");
    for (_, app) in &data.apps {
        println!("  {}", app.name);
        
        // Show active profile or legacy directory
        if let Some(active_profile) = app.profiles.iter().find(|p| p.active) {
            println!("    {:?}: {}", active_profile.profile_type, active_profile.location.display());
        } else if let Some(ref dir) = app.directory {
            println!("    Directory: {}", dir.display());
        }
        
        if !app.tags.is_empty() {
            println!("    Tags: {}", app.tags.join(", "));
        }
        if let Some(ref repo) = app.github_repo {
            println!("    GitHub: {}", repo);
        }
        println!("    Created: {}", format_datetime(&app.created_at));
        println!();
    }
    
    Ok(())
}

fn get_app_info(search_term: &str) -> Result<()> {
    let data = load_data()?;
    
    if data.apps.is_empty() {
        println!("No apps found.");
        return Ok(());
    }
    
    let app = find_app_by_name(&data, search_term);
    
    match app {
        Some(app) => {
            println!("{}", app.name);
            
            // Show profiles
            if !app.profiles.is_empty() {
                println!("  Profiles:");
                for profile in &app.profiles {
                    let active_marker = if profile.active { " (active)" } else { "" };
                    println!("    {:?}: {}{}", profile.profile_type, profile.location.display(), active_marker);
                    if let Some(ref machine) = profile.machine_name {
                        println!("      Machine: {}", machine);
                    }
                    if let Some(ref notes) = profile.notes {
                        println!("      Notes: {}", notes);
                    }
                }
            }
            
            // Show legacy directory if no profiles
            if app.profiles.is_empty() {
                if let Some(dir) = &app.directory {
                    println!("  Directory: {}", dir.display());
                }
            }
            
            if !app.tags.is_empty() {
                println!("  Tags: {}", app.tags.join(", "));
            }
            if let Some(ref repo) = app.github_repo {
                println!("  GitHub: {}", repo);
            }
            println!("  Created: {}", format_datetime(&app.created_at));
            println!("  Updated: {}", format_datetime(&app.updated_at));
        }
        None => {
            println!("App '{}' not found.", search_term);
        }
    }
    
    Ok(())
}

fn remove_app(search_term: &Option<String>, use_current_dir: bool) -> Result<()> {
    let mut data = load_data()?;
    
    if data.apps.is_empty() {
        println!("No apps found.");
        return Ok(());
    }
    
    let app = if use_current_dir {
        find_app_by_current_dir(&data)?
    } else {
        match search_term {
            Some(term) => find_app_by_name(&data, term),
            None => return Err(anyhow::anyhow!("Either --get or --current-dir must be specified")),
        }
    };
    
    match app {
        Some(app) => {
            println!("Found app: {}", app.name);
            
            if let Some(active_profile) = app.profiles.iter().find(|p| p.active) {
                println!("  {:?}: {}", active_profile.profile_type, active_profile.location.display());
            } else if let Some(dir) = &app.directory {
                println!("  Directory: {}", dir.display());
            }
            
            if !app.tags.is_empty() {
                println!("  Tags: {}", app.tags.join(", "));
            }
            println!();
            
            print!("Are you sure you want to remove this app? (y/N): ");
            io::stdout().flush()?;
            
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();
            
            if input == "y" || input == "yes" {
                let app_name = app.name.clone();
                data.apps.remove(&app_name);
                save_data(&data)?;
                println!("Removed app: {}", app_name);
            } else {
                println!("Removal cancelled.");
            }
        }
        None => {
            if use_current_dir {
                println!("No app found for current directory.");
            } else if let Some(term) = search_term {
                println!("App '{}' not found.", term);
            }
        }
    }
    
    Ok(())
}

fn add_profile(app: &mut App, profile_type: ProfileType, location: PathBuf, machine: Option<String>, notes: Option<String>) -> Result<()> {
    // Check if profile type already exists
    if app.profiles.iter().any(|p| p.profile_type == profile_type) {
        return Err(anyhow::anyhow!("Profile type {:?} already exists for this app", profile_type));
    }
    
    let is_first_profile = app.profiles.is_empty();
    
    app.profiles.push(AppProfile {
        profile_type,
        location,
        machine_name: machine,
        notes,
        active: is_first_profile, // First profile is active by default
    });
    
    app.updated_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    Ok(())
}

fn list_profiles(app: &App) {
    if app.profiles.is_empty() {
        println!("No profiles found for app: {}", app.name);
        if let Some(ref dir) = app.directory {
            println!("  Legacy directory: {}", dir.display());
        }
        return;
    }
    
    println!("Profiles for app: {}", app.name);
    for profile in &app.profiles {
        let active_marker = if profile.active { " (active)" } else { "" };
        println!("  {:?}: {}{}", profile.profile_type, profile.location.display(), active_marker);
        if let Some(ref machine) = profile.machine_name {
            println!("    Machine: {}", machine);
        }
        if let Some(ref notes) = profile.notes {
            println!("    Notes: {}", notes);
        }
    }
}

fn activate_profile(app: &mut App, profile_type: ProfileType) -> Result<()> {
    let mut found = false;
    for profile in &mut app.profiles {
        if profile.profile_type == profile_type {
            profile.active = true;
            found = true;
        } else {
            profile.active = false;
        }
    }
    
    if !found {
        return Err(anyhow::anyhow!("Profile type {:?} not found for this app", profile_type));
    }
    
    app.updated_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    Ok(())
}

fn remove_profile(app: &mut App, profile_type: ProfileType) -> Result<()> {
    let initial_len = app.profiles.len();
    app.profiles.retain(|p| p.profile_type != profile_type);
    
    if app.profiles.len() == initial_len {
        return Err(anyhow::anyhow!("Profile type {:?} not found for this app", profile_type));
    }
    
    // If we removed the active profile, make the first remaining profile active
    if !app.profiles.is_empty() && !app.profiles.iter().any(|p| p.active) {
        app.profiles[0].active = true;
    }
    
    app.updated_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    Ok(())
}

fn load_data() -> Result<AppsData> {
    let data_file = get_data_file_path();
    if !data_file.exists() {
        return Ok(AppsData::default());
    }
    
    let content = fs::read_to_string(&data_file)?;
    let mut data: AppsData = serde_json::from_str(&content)?;
    
    // Migrate legacy directory field to profiles if needed
    for app in data.apps.values_mut() {
        if app.profiles.is_empty() && app.directory.is_some() {
            app.profiles.push(AppProfile {
                profile_type: ProfileType::Dev,
                location: app.directory.as_ref().unwrap().clone(),
                machine_name: None,
                notes: None,
                active: true,
            });
        }
    }
    
    Ok(data)
}

fn save_data(data: &AppsData) -> Result<()> {
    let content = serde_json::to_string_pretty(data)?;
    // Ensure the directory exists
    if let Some(parent) = get_data_file_path().parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(get_data_file_path(), content)?;
    Ok(())
}

fn find_app_by_name<'a>(data: &'a AppsData, search_term: &str) -> Option<&'a App> {
    let search_lower = search_term.to_lowercase();
    
    // First try exact match (case insensitive)
    for (name, app) in &data.apps {
        if name.to_lowercase() == search_lower {
            return Some(app);
        }
    }
    
    // Then try fuzzy matching: remove spaces, hyphens, underscores
    let normalized_search = normalize_name(&search_lower);
    
    for (name, app) in &data.apps {
        let normalized_name = normalize_name(&name.to_lowercase());
        if normalized_name == normalized_search {
            return Some(app);
        }
    }
    
    // Finally try contains match
    for (name, app) in &data.apps {
        let normalized_name = normalize_name(&name.to_lowercase());
        if normalized_name.contains(&normalized_search) || normalized_search.contains(&normalized_name) {
            return Some(app);
        }
    }
    
    None
}

fn find_app_by_name_mut<'a>(data: &'a mut AppsData, search_term: &str) -> Option<&'a mut App> {
    let search_lower = search_term.to_lowercase();
    
    // Collect keys to avoid borrow checker issues
    let keys: Vec<String> = data.apps.keys().cloned().collect();
    
    // Try exact match first
    for name in &keys {
        if name.to_lowercase() == search_lower {
            return data.apps.get_mut(name);
        }
    }
    
    // Then try fuzzy matching
    let normalized_search = normalize_name(&search_lower);
    for name in &keys {
        let normalized_name = normalize_name(&name.to_lowercase());
        if normalized_name == normalized_search {
            return data.apps.get_mut(name);
        }
    }
    
    // Finally try contains match
    for name in &keys {
        let normalized_name = normalize_name(&name.to_lowercase());
        if normalized_name.contains(&normalized_search) || normalized_search.contains(&normalized_name) {
            return data.apps.get_mut(name);
        }
    }
    
    None
}

fn find_app_by_current_dir(data: &AppsData) -> Result<Option<&App>> {
    let current_dir = std::env::current_dir()?;
    
    for (_, app) in &data.apps {
        // Check profiles first
        for profile in &app.profiles {
            if profile.location == current_dir {
                return Ok(Some(app));
            }
        }
        
        // Check legacy directory field
        if let Some(app_dir) = &app.directory {
            if app_dir == &current_dir {
                return Ok(Some(app));
            }
        }
    }
    
    Ok(None)
}

fn normalize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

fn format_datetime(datetime_str: &str) -> String {
    if let Ok(dt) = DateTime::parse_from_rfc3339(datetime_str) {
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        datetime_str.to_string()
    }
}

fn get_machine_name() -> Option<String> {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .ok()
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|output| {
                    if output.status.success() {
                        String::from_utf8(output.stdout)
                            .ok()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                    } else {
                        None
                    }
                })
        })
}