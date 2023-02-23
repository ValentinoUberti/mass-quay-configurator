use super::organization_struct::{OrganizationYaml, QuayResponse};
use crate::quay_configurator::organization_struct::{Actions, QuayFnArguments};
use array_tool::vec::Uniq;
use futures::future::join_all;
use governor::clock::{QuantaClock, QuantaInstant};
use governor::middleware::NoOpMiddleware;
use governor::state::{InMemoryState, NotKeyed};
use governor::{self, RateLimiter};
use log::{error, info, warn};
use question::{Answer, Question};
use reqwest::StatusCode;
use rpassword::read_password;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::error::Error;
use std::io::{self, Write};
use std::num::NonZeroU32;
use std::path::Path;
use std::process;
use std::{fs::File, sync::Arc};
use tokio::fs::{self, read_dir};
use tokio::time::Instant;

// Main Config struct
#[derive(Debug)]
pub struct QuayXmlConfig {
    organization: Vec<OrganizationYaml>,
    directory: String,
    governor: Arc<RateLimiter<NotKeyed, InMemoryState, QuantaClock, NoOpMiddleware<QuantaInstant>>>,
    log_level: log::Level,
    log_verbosity: u8,
    quay_login_configs: QuayLoginConfigs,
    timeout: u64,
    tls_verify: bool,
    retries: u32,
}

impl QuayXmlConfig {
    pub fn new(
        directory: &String,
        req_per_seconds: u32,
        log_level: log::Level,
        log_verbosity: u8,
        timeout: u64,
        ignore_login_config: bool,
        tls_verify: bool,
        retries: u32,
    ) -> Result<Self, Box<dyn Error>> {
        let governor = Arc::new(governor::RateLimiter::direct(governor::Quota::per_second(
            NonZeroU32::new(req_per_seconds).unwrap(),
        )));

        if !ignore_login_config {
            let quay_configs_file = File::open(".mqc/login.yaml")?;
            match serde_yaml::from_reader(quay_configs_file) {
                Ok(quay_login_configs) => {
                    return Ok(Self {
                        organization: vec![],
                        directory: directory.clone(),
                        governor,
                        log_level,
                        log_verbosity,
                        timeout,
                        tls_verify,
                        quay_login_configs,
                        retries,
                    });
                }
                Err(e) => return Err(Box::new(e)),
            }
        } else {
            // Creating dummy configs
            let quay_login_configs: QuayLoginConfigs = QuayLoginConfigs {
                quay_endpoint_login: vec![],
                mirror_repository: None,
            };
            return Ok(Self {
                organization: vec![],
                directory: directory.clone(),
                governor,
                log_level,
                log_verbosity,
                timeout,
                tls_verify,
                quay_login_configs,
                retries,
            });
        }
    }
    pub async fn load_config(&mut self) -> Result<(), std::io::Error> {
        let mut files = read_dir(self.directory.to_owned()).await?;

        while let Some(f) = files.next_entry().await? {
            info!("Loading config from  {:?} ", f.file_name());

            match f
                .path()
                .extension()
                .unwrap_or_default()
                .to_str()
                .unwrap_or_default()
            {
                "yaml" | "yml" => {
                    let yaml_file = File::open(f.path())?;

                    match serde_yaml::from_reader(yaml_file) {
                        Ok(scrape_config) => {
                            //error!("Opening file {:?}: Ignoring...",f.file_name());

                            self.organization.push(scrape_config);
                        }
                        Err(e) => {
                            warn!("Opening file {:?}: {:?}. Ignoring...", f.file_name(), e)
                        }
                    }
                }
                _ => {
                    warn!("Opening file {:?}: Ignoring...", f.path())
                }
            }
        }

        // Adds optional endpoints (replicated_to -> endpoints)
        // Warn if the replicated endpoints already exists as a main Quay endpoint.
        // Only unique endpoints are considered

        let tmp_organization = self.organization.clone();

        for org in &tmp_organization {
            match &org.replicate_to {
                Some(replicated_to) => {
                    for endpoint in replicated_to {
                        let mut new_org = org.clone();
                        new_org.change_endpoint(endpoint.to_string());
                        if !self.organization.contains(&new_org) {
                            self.organization.push(new_org);
                        } else {
                            let str_error=format!("Endpoint replication '{}' already attached to the Quay organization '{}' with endpoint '{}'. Ignoring....",endpoint,new_org.quay_organization,new_org.quay_endpoint);
                            warn!("{}", str_error);
                        }
                    }
                }
                None => {}
            }
        }

        Ok(())
    }

    pub async fn write_log(log_verbosity: u8, message: &str) {
        if log_verbosity >= 5 {
            info!("{}", message);
        }
    }
    pub async fn check_config(&self, halt_on_error: bool) -> Result<(), std::io::Error> {

        
        let mut files = read_dir(self.directory.to_owned()).await?;
        while let Some(f) = files.next_entry().await? {
            match f
                .path()
                .extension()
                .unwrap_or_default()
                .to_str()
                .unwrap_or_default()
            {
                "yaml" | "yml" => {
                    let f2 = File::open(f.path())?;

                    let result: Result<OrganizationYaml, serde_yaml::Error> =
                        serde_yaml::from_reader(f2);
                    match result {
                        Ok(org) => {
                            info!("Config syntax of {:?} verified.  ", f.file_name());

                            let mut quay_endpoints: Vec<String> = Vec::new();
                            let mut quay_mirror_login = QuayMirrorLogin::default();

                            //Mirror login repository not present in login.yaml
                            let mut present_quay_mirror_login = QuayMirrorLogin::default();

                            //println!("HERE");

                            quay_endpoints.push(org.quay_endpoint.clone());

                            match org.replicate_to {
                                Some(replicated_to) => {
                                    quay_endpoints.extend(replicated_to);
                                }
                                None => {}
                            }

                            // Extract repositories mirror login informations
                            for repo in &org.repositories {
                                match &repo.mirror_params {
                                    Some(mirror_params) => match &mirror_params
                                        .ext_registry_username
                                    {
                                        Some(username) => {
                                            let mirror_login = MirrorLogin {
                                                organization: org.quay_organization.clone(),
                                                repository: repo.name.clone(),
                                                ext_registry_username: username.to_string(),
                                                ext_registry_password: "".to_string(),
                                            };
                                            quay_mirror_login.mirror_repository.push(mirror_login);
                                        }
                                        None => {}
                                    },

                                    None => {}
                                }
                            }

                            let msg =
                                &format!("Mirror repository wanted user {:?}", quay_mirror_login);
                            Self::write_log(self.log_verbosity, &msg).await;

                            //println!("WANTED {:?}", quay_mirror_login);

                            match &self.quay_login_configs.mirror_repository {
                                Some(actuals) => {
                                    for actual in actuals {
                                        for configured in &quay_mirror_login.mirror_repository {
                                            if actual.ext_registry_username
                                                == configured.ext_registry_username
                                            {
                                                if actual.repository == configured.repository {
                                                    if actual.organization == org.quay_organization
                                                    {
                                                        if actual.ext_registry_password.len() > 0 {
                                                            present_quay_mirror_login
                                                                .mirror_repository
                                                                .push(configured.clone());
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                None => {}
                            } // Match

                            quay_endpoints = quay_endpoints.unique();

                            let msg =
                                &format!("Found {} unique Quay endpoint(s)", quay_endpoints.len());
                            Self::write_log(self.log_verbosity, &msg).await;

                            for q in &quay_endpoints {
                                if let Some(_t) = self
                                    .quay_login_configs
                                    .get_token_from_quay_endopoint(q.to_string())
                                {
                                } else {
                                    let err_str = format!("No token found for {} Quay endpoint. Please run `mqc login`. Ignoring this Quay endpoint.",q.to_string());
                                    error!("{}", err_str);
                                    continue;
                                }
                            }

                            // Exit if halt_on_error==true.

                            // Calculating vector difference
                            // if present_quay_mirror_login and quay_mirror_login Vector are different, there is no password in login.yaml for required repo.
                            // It's a O(N*M) operation, ok for small vectors

                            let s1: HashSet<MirrorLogin> = present_quay_mirror_login
                                .mirror_repository
                                .iter()
                                .cloned()
                                .collect();
                            let s2: HashSet<MirrorLogin> = quay_mirror_login
                                .mirror_repository
                                .iter()
                                .cloned()
                                .collect();

                            let difference: HashSet<MirrorLogin> =
                                (&s2 - &s1).iter().cloned().collect();

                            if difference.len() > 0 {
                                let msg = &format!("Missing mirror user password: {:?}. Check .mqc/login.yaml or run login subcommand.", difference);
                                error!("{}", msg);
                                if halt_on_error {
                                    std::process::exit(1);
                                }
                            } else {
                                let msg =
                                    &format!("Missing mirror user password: {:?}", difference);
                                Self::write_log(self.log_verbosity, &msg).await;
                            }
                        } // OK
                        Err(e) => {
                            error!("{:?}", e)
                        }
                    }
                } // match f yaml

                _ => {
                    warn!("Opening file {:?}: Ignoring...", f.path())
                }
            } // match
        } // while

        

        Ok(())
    }

    pub async fn create_login(self) -> Result<(), Box<dyn Error>> {
        let mut quay_endpoints: Vec<String> = Vec::new();
        let mut quay_mirror_login = QuayMirrorLogin::default();

        // Extract organizations details from yaml
        for org in self.organization {
            quay_endpoints.push(org.quay_endpoint.clone());

            match org.replicate_to {
                Some(replicated_to) => {
                    quay_endpoints.extend(replicated_to);
                }
                None => {}
            }

            // Extract repositories mirror login informations
            for repo in org.repositories {
                match repo.mirror_params {
                    Some(mirror_params) => match mirror_params.ext_registry_username {
                        Some(username) => {
                            let mirror_login = MirrorLogin {
                                organization: org.quay_organization.clone(),
                                repository: repo.name,
                                ext_registry_username: username,
                                ext_registry_password: "".to_string(),
                            };

                            quay_mirror_login.mirror_repository.push(mirror_login);
                        }
                        None => {}
                    },

                    None => {}
                }
            } // for
        }

        quay_mirror_login.mirror_repository = quay_mirror_login.mirror_repository.unique();

        let msg = &format!("Found {:?} Quay mirror login", quay_mirror_login);
        Self::write_log(self.log_verbosity, &msg).await;

        quay_endpoints = quay_endpoints.unique();

        let msg = &format!("Found {} unique Quay endpoint(s)", quay_endpoints.len());
        Self::write_log(self.log_verbosity, &msg).await;

        // Checking if .mqc directory exists and creating it if does not.

        let login_directory = ".mqc";
        let login_file = "login.yaml";

        if !Path::new(login_directory).is_dir() {
            let msg = &format!(".mqc directory does not exists. Creating...");
            Self::write_log(self.log_verbosity, &msg).await;

            fs::create_dir(login_directory).await?;

            let msg = &format!(".mqc directory created.");
            Self::write_log(self.log_verbosity, &msg).await;
        } else {
            let msg = &format!(".mqc directory exists.");
            Self::write_log(self.log_verbosity, &msg).await;
        }

        let relative_login_file_path = format!("{}/{}", login_directory, login_file);
        if std::path::Path::new(&relative_login_file_path).exists() {
            match Question::new(".mqc/login.yaml exists: Do you want to recreate it?")
                .yes_no()
                .until_acceptable()
                .default(Answer::YES)
                .show_defaults()
                .ask()
            {
                Some(answer) => match answer {
                    Answer::RESPONSE(_) => {}
                    Answer::NO => {}
                    Answer::YES => fs::remove_file(&relative_login_file_path).await?,
                },
                None => todo!(),
            }
        }
        // To do - check path for windows
        if !std::path::Path::new(&relative_login_file_path).exists() {
            warn!("{} does not exits. Creating...", login_file);

            let mut logins = QuayLoginConfigs::default();

            for q in quay_endpoints {
                print!("Please insert token for {}: ", q);
                io::stdout().flush()?;

                //let mut token = String::new();
                //io::stdin().read_line(&mut token)?;

                let token = read_password()?;

                let endpoint = QuayEndopoint {
                    quay_endpoint: q,
                    quay_token: token.trim().to_string(),
                };

                logins.quay_endpoint_login.push(endpoint);
            }

            let mut tmp_quay_mirror_login = QuayMirrorLogin::default();

            for mirror in quay_mirror_login.mirror_repository {
                print!(
                    "Please insert password for user '{}', repository '{}' of organization '{}' :",
                    mirror.ext_registry_username, mirror.repository, mirror.organization
                );
                io::stdout().flush()?;

                //let mut token = String::new();
                //io::stdin().read_line(&mut token)?;

                let token = read_password()?;

                let single_mirror_login = MirrorLogin {
                    organization: mirror.organization,
                    repository: mirror.repository,
                    ext_registry_username: mirror.ext_registry_username,
                    ext_registry_password: token.trim().to_string(),
                };

                tmp_quay_mirror_login
                    .mirror_repository
                    .push(single_mirror_login);
            } // for

            let f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .append(true)
                .open(&relative_login_file_path)?;

            logins.mirror_repository = Some(tmp_quay_mirror_login.mirror_repository);

            serde_yaml::to_writer(f, &logins)?;
        }

        Ok(())
    }

    pub fn get_organizations(&self) -> &Vec<OrganizationYaml> {
        &self.organization
    }

    pub fn get_cloned_governor(
        &self,
    ) -> Arc<RateLimiter<NotKeyed, InMemoryState, QuantaClock, NoOpMiddleware<QuantaInstant>>> {
        self.governor.clone()
    }

    fn print_result(&self, _description: String, result: Result<QuayResponse, Box<dyn Error>>) {
        match result {
            Ok(r) => {
                let mut corrected_description = String::new();

                if r.status_code == StatusCode::NO_CONTENT {
                    // 204 from success delete organization
                    corrected_description.insert_str(0, "No Content success");
                } else {
                    corrected_description.insert_str(0, r.description.as_str());
                }
                if self.log_verbosity >= 5 {
                    if r.status_code.is_success() {
                        info!("{:?}", r);
                    } else {
                        warn!("{:?}", r);
                    }
                }
                //println!("------------------------");
                //println!("{} {}", description, corrected_description);
                //println!("Status code: {}", r.status_code);
                //println!("Message: {}", r.response);
            }
            Err(e) => {
                error!("Error: {}", e);
                panic!(
                    "{}",
                    format!("Can not continue. Please check network connectivity")
                );
            }
        }
    }

    pub async fn delete_all(&self) -> Result<(), Box<dyn Error>> {
        let mut handles_delete_organization = Vec::new();
        let orgs = self.get_organizations();

        for org in orgs {
            info!("Processing organization: {}", org.quay_organization);

            let token: String;

            if let Some(t) = self
                .quay_login_configs
                .get_token_from_quay_endopoint(org.get_quay_endpoint())
            {
                token = t
            } else {
                let err_str = format!("No token found for {} Quay endpoint. Please run `mqc login`. Ignoring this Quay organization.",org.get_quay_endpoint());
                error!("{}", err_str);
                continue;
            }

            let quay_fn_arguments = QuayFnArguments {
                token,
                governor: self.get_cloned_governor(),
                log_level: self.log_level,
                log_verbosity: self.log_verbosity,
                timeout: self.timeout,
                tls_verify: self.tls_verify,
                mirror_login: None,
                retries: self.retries,
            };

            handles_delete_organization.push(org.delete_organization(quay_fn_arguments));
        }
        let now = Instant::now();

        info!(
            "Deleting {} organization...",
            handles_delete_organization.len()
        );
        let results = join_all(handles_delete_organization);

        for result in results.await {
            self.print_result("Organization ->".to_string(), result);
        }

        info!(
            "Organizations deleted in {} seconds.",
            now.elapsed().as_secs_f32()
        );

        Ok(())
    }

    pub async fn create_all(&self) -> Result<(), Box<dyn Error>> {
        let mut handles_all_organizations = Vec::new();
        // let mut handles_delete_organization = Vec::new();
        let mut handles_all_robots = Vec::new();
        let mut handles_all_teams = Vec::new();
        let mut handles_all_repositories = Vec::new();
        let mut handles_all_repositories_permissions = Vec::new();
        let mut handles_all_team_members = Vec::new();
        let mut handles_all_extra_user_permissions = Vec::new();
        let mut handles_all_extra_team_permissions = Vec::new();
        let mut handles_all_mirror_configurations = Vec::new();
        let mut handles_all_mirror_sync_configurations = Vec::new();

        let orgs = self.get_organizations();

        for org in orgs {
            info!(
                "Processing config for organization: {}",
                org.quay_organization
            );

            let token: String;

            if let Some(t) = self
                .quay_login_configs
                .get_token_from_quay_endopoint(org.get_quay_endpoint())
            {
                token = t
            } else {
                let err_str = format!("No token found for {} Quay endpoint. Please run `mqc login`. Ignoring this Quay organization.",org.get_quay_endpoint());
                error!("{}", err_str);
                continue;
            }

            let mut tmp_mirror_login = vec![];

            match &self.quay_login_configs.mirror_repository {
                Some(login_vec) => {
                    for l in login_vec {
                        if l.organization == org.quay_organization {
                            tmp_mirror_login.push(l.clone());
                        }
                    }
                }
                None => {}
            };

            let quay_fn_arguments = QuayFnArguments {
                token,
                governor: self.get_cloned_governor(),
                log_level: self.log_level,
                log_verbosity: self.log_verbosity,
                timeout: self.timeout,
                tls_verify: self.tls_verify,
                mirror_login: Some(tmp_mirror_login),
                retries: self.retries,
            };

            handles_all_organizations.push(org.create_organization(quay_fn_arguments.clone()));

            if let Some(robots) = &org.robots {
                for robot in robots {
                    handles_all_robots.push(org.create_robot(robot, quay_fn_arguments.clone()));
                }
            }

            if let Some(teams) = &org.teams {
                for team in teams {
                    handles_all_teams.push(org.create_team(team, quay_fn_arguments.clone()));

                    if let Some(team_name) = &team.name {
                        if let Some(users_member) = &team.members.users {
                            for member in users_member {
                                handles_all_team_members.push(org.add_user_to_team(
                                    &team_name,
                                    &member,
                                    quay_fn_arguments.clone(),
                                ))
                            }
                        }

                        if let Some(robots_member) = &team.members.robots {
                            for member in robots_member {
                                handles_all_team_members.push(org.add_robot_to_team(
                                    &team_name,
                                    &member,
                                    quay_fn_arguments.clone(),
                                ))
                            }
                        }

                        if let Some(_groupdn) = &team.groupdn {
                            handles_all_mirror_sync_configurations
                                .push(org.create_team_sync(&team, quay_fn_arguments.clone()));
                        }
                    } // Team name
                }
            } // Teams
            for repository in &org.repositories {
                handles_all_repositories
                    .push(org.create_repository(repository, quay_fn_arguments.clone()));
                handles_all_extra_user_permissions.push(
                    org.get_user_permission_from_repository(&repository, quay_fn_arguments.clone()),
                );
                handles_all_extra_team_permissions.push(
                    org.get_team_permission_from_repository(&repository, quay_fn_arguments.clone()),
                );

                handles_all_mirror_configurations
                    .push(org.create_repository_mirror(&repository, quay_fn_arguments.clone()));

                if let Some(permissions) = &repository.permissions {
                    if let Some(robots) = &permissions.robots {
                        for robot in robots {
                            handles_all_repositories_permissions.push(
                                org.grant_robot_permission_to_repository(
                                    &repository.name,
                                    &robot,
                                    quay_fn_arguments.clone(),
                                ),
                            )
                        }
                    }

                    for team in &permissions.teams {
                        for t in team {
                            handles_all_repositories_permissions.push(
                                org.grant_team_permission_to_repository(
                                    &repository.name,
                                    &t,
                                    quay_fn_arguments.clone(),
                                ),
                            )
                        }
                    }

                    if let Some(users) = &permissions.users {
                        for user in users {
                            handles_all_repositories_permissions.push(
                                org.grant_user_permission_to_repository(
                                    &repository.name,
                                    &user,
                                    quay_fn_arguments.clone(),
                                ),
                            )
                        }
                    }
                }
            }
        }

        let total_requests = handles_all_organizations.len()
            + handles_all_mirror_sync_configurations.len()
            + handles_all_robots.len()
            + handles_all_teams.len()
            + handles_all_repositories.len()
            + (handles_all_repositories_permissions.len() * 2)
            + handles_all_team_members.len()
            + handles_all_extra_user_permissions.len()
            + handles_all_extra_team_permissions.len()
            + (handles_all_mirror_configurations.len() * 3);

        info!("TOTAL REQUESTS : {}", total_requests);

        if total_requests == 0 {
            error!("Can not continue");
            process::exit(1);
        }

        // Create organization
        info!(
            "Creating {} organization...",
            handles_all_organizations.len()
        );

        let results = join_all(handles_all_organizations);

        let now = Instant::now();
        for result in results.await {
            self.print_result("Organization ->".to_string(), result);
        }

        info!(
            "Organizations created in  {} seconds.",
            now.elapsed().as_secs_f32()
        );

        // Create robots
        info!("Creating {} robots...", handles_all_robots.len());

        let now = Instant::now();
        let results = join_all(handles_all_robots);

        for result in results.await {
            self.print_result("Robots ->".to_string(), result);
        }

        info!(
            "Robots created in  {} seconds.",
            now.elapsed().as_secs_f32()
        );
        // Create teams
        info!("Creating {} teams...", handles_all_teams.len());

        let now = Instant::now();

        let results = join_all(handles_all_teams);

        for result in results.await {
            self.print_result("Teams ->".to_string(), result);
        }

        info!("Teams created in  {} seconds.", now.elapsed().as_secs_f32());

        // Adding team members
        info!("Adding {} team members...", handles_all_team_members.len());
        let now = Instant::now();
        let results = join_all(handles_all_team_members);

        for result in results.await {
            self.print_result("Team members ->".to_string(), result);
        }

        info!(
            "Teams members added in  {} seconds.",
            now.elapsed().as_secs_f32()
        );

        // Activating team sync  handles_all_mirror_sync_configurations

        info!(
            "Configuring {} team ldap sync...",
            handles_all_mirror_sync_configurations.len()
        );
        let now = Instant::now();
        let results = join_all(handles_all_mirror_sync_configurations);

        for result in results.await {
            self.print_result("Team ldap sync ->".to_string(), result);
        }

        info!(
            "Teams sync activated  {} seconds.",
            now.elapsed().as_secs_f32()
        );

        // Create repositories
        info!(
            "Creating {} repositories...",
            handles_all_repositories.len()
        );

        let now = Instant::now();
        let results = join_all(handles_all_repositories);

        for result in results.await {
            self.print_result("Repository ->".to_string(), result);
        }

        info!(
            "Repositories created in  {} seconds.",
            now.elapsed().as_secs_f32()
        );
        //  Get user currently repositories permission (IF ANY)
        info!(
            "Delete extra user and robot permission from {} repository...",
            handles_all_extra_user_permissions.len()
        );
        let now = Instant::now();
        let results = join_all(handles_all_extra_user_permissions);

        for result in results.await {
            self.print_result("Repository USER permissions ->".to_string(), result);
        }

        info!(
            "Extra users and robots permissions deleted in  {} seconds.",
            now.elapsed().as_secs_f32()
        );
        // Get currently team repositories permission (IF ANY)
        info!(
            "Delete extra team permission from {} repository...",
            handles_all_extra_team_permissions.len()
        );
        let now = Instant::now();
        let results = join_all(handles_all_extra_team_permissions);

        for result in results.await {
            self.print_result("Repository TEAM permissions ->".to_string(), result);
        }

        info!(
            "Extra teams permissions deleted in  {} seconds.",
            now.elapsed().as_secs_f32()
        );

        // Create repositories permission
        info!(
            "Creating {} repositories permissions...",
            handles_all_repositories_permissions.len()
        );

        let now = Instant::now();
        let results = join_all(handles_all_repositories_permissions);

        for result in results.await {
            self.print_result("Repository permissions ->".to_string(), result);
        }

        info!(
            "Repositories permissions created in  {} seconds.",
            now.elapsed().as_secs_f32()
        );

        // Configure repository mirror
        info!(
            "Configuring {} repositories mirror...",
            handles_all_mirror_configurations.len()
        );

        let now = Instant::now();
        let results = join_all(handles_all_mirror_configurations);

        for result in results.await {
            self.print_result("Repository mirror ->".to_string(), result);
        }
        info!(
            "Repositories mirror configured in  {} seconds.",
            now.elapsed().as_secs_f32()
        );

        Ok(())
        /*

        */
    }
}

// Configuration struct contaning oauth token for each Quay endpoints
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
struct QuayLoginConfigs {
    pub quay_endpoint_login: Vec<QuayEndopoint>,
    pub mirror_repository: Option<Vec<MirrorLogin>>,
}

impl QuayLoginConfigs {
    fn get_quay_login_configs(&self) -> Vec<QuayEndopoint> {
        self.quay_endpoint_login.clone()
    }

    pub fn get_token_from_quay_endopoint(&self, endpoint: String) -> Option<String> {
        for configured_endpoint in self.get_quay_login_configs() {
            if configured_endpoint.quay_endpoint == endpoint {
                if configured_endpoint.quay_token.len() > 1 {
                    return Some(configured_endpoint.quay_token);
                } else {
                    return None;
                }
            }
        }

        None
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
struct QuayEndopoint {
    pub quay_endpoint: String,
    pub quay_token: String,
}

// Configuration struct for saving mirroring password for each repositories (if mirror username exists)
// ```
// repositories:
//  - name: alpine
//  visibility: "public"
//  description: "mirror of **quay.io/libpod/alpine**, mirrored tags: latest, v1"
//  mirror: true
//  mirror_params:
//      src_registry: quay.io
//      src_image: libpod/alpine
//      src_image_tags:
//        - "latest"
//        - "v1"
//      ext_registry_username: "valeidm"
// ```
// if "ext_registry_username" a relative password is saved in .mqc/login.yaml
//
// login.yaml
//
// quay_mirror_login:
//   - organization: <quay-organization>
//     repository: <quay-repository>
//     ext_registry_username: <ext_registry_username>
//     ext_registry_password:
//

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct QuayMirrorLogin {
    pub mirror_repository: Vec<MirrorLogin>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Hash)]
pub struct MirrorLogin {
    pub organization: String,
    pub repository: String,
    pub ext_registry_username: String,
    pub ext_registry_password: String,
}
