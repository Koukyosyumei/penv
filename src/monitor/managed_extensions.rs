use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

const HOST: &str = include_str!("managed_extension_host.cs");
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Compiler {
    root: PathBuf,
    dotnet: PathBuf,
    framework: String,
    host: PathBuf,
    next: usize,
}

impl Compiler {
    pub(crate) fn new() -> Result<Self, String> {
        let dotnet = std::env::var_os("PENV_DOTNET")
            .map(PathBuf::from)
            .unwrap_or_else(|| "dotnet".into());
        let version = run(Command::new(&dotnet).arg("--version"))?;
        let major = version
            .trim()
            .split('.')
            .next()
            .and_then(|part| part.parse::<u32>().ok())
            .ok_or("invalid .NET SDK version")?;
        let root = loop {
            let sequence = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let candidate = std::env::temp_dir()
                .join(format!("penv-extensions-{}-{sequence}", std::process::id()));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.to_string()),
            }
        };
        let mut compiler = Self {
            root,
            dotnet,
            framework: format!("net{major}.0"),
            host: PathBuf::new(),
            next: 0,
        };
        let host_project = compiler.root.join("host");
        fs::create_dir(&host_project).map_err(|e| e.to_string())?;
        fs::write(host_project.join("Host.cs"), HOST).map_err(|e| e.to_string())?;
        compiler.host = compiler.build(&host_project, "exe", "")?;
        Ok(compiler)
    }

    pub(crate) fn compile(&mut self, sources: &[PathBuf]) -> Result<PathBuf, String> {
        let project = self.root.join(format!("extension-{}", self.next));
        self.next += 1;
        fs::create_dir(&project).map_err(|e| e.to_string())?;
        // The managed host path can be passed as an MSBuild property without
        // interpolating it into the project XML.
        for (index, source) in sources.iter().enumerate() {
            fs::copy(source, project.join(format!("Source{index}.cs")))
                .map_err(|e| format!("{}: {e}", source.display()))?;
        }
        self.build(&project, "library", r#"<ItemGroup><Reference Include="ManagedExtensionHost"><HintPath>$(PenvExtensionHost)</HintPath><Private>false</Private></Reference></ItemGroup>"#)
    }

    fn build(&self, directory: &Path, output_type: &str, extra: &str) -> Result<PathBuf, String> {
        let assembly = if output_type == "exe" {
            "ManagedExtensionHost"
        } else {
            directory
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("invalid extension directory name")?
        };
        let project = directory.join("Project.csproj");
        fs::write(&project, format!(
            "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>{}</TargetFramework><OutputType>{output_type}</OutputType><AssemblyName>{assembly}</AssemblyName><NuGetAudit>false</NuGetAudit></PropertyGroup>{extra}</Project>",
            self.framework,
        )).map_err(|e| e.to_string())?;
        let mut command = Command::new(&self.dotnet);
        command
            .arg("build")
            .arg(&project)
            .args(["--nologo", "--verbosity", "quiet", "-o"])
            .arg(directory.join("out"));
        if !self.host.as_os_str().is_empty() {
            command.arg(format!("-p:PenvExtensionHost={}", self.host.display()));
        }
        run(&mut command)?;
        fs::canonicalize(directory.join("out").join(format!("{assembly}.dll")))
            .map_err(|e| e.to_string())
    }

    pub(crate) fn scan(&self, assembly: &Path) -> Result<Metadata, String> {
        let text = run(Command::new(&self.dotnet)
            .arg(&self.host)
            .arg("scan")
            .arg(assembly))?;
        let mut result = Metadata::default();
        for line in text.lines() {
            if let Some((kind, value)) = line.split_once('\t') {
                match kind {
                    "T" => {
                        result.types.insert(value.to_owned());
                    }
                    "M" => {
                        result.methods.insert(value.to_owned());
                    }
                    _ => return Err("invalid managed extension metadata".into()),
                }
            }
        }
        Ok(result)
    }

    pub(crate) fn invoke(&self, assembly: &Path, method: &str) -> Result<String, String> {
        run(Command::new(&self.dotnet)
            .arg(&self.host)
            .arg("invoke")
            .arg(assembly)
            .arg(method))
    }
}

#[derive(Default)]
pub(crate) struct Metadata {
    pub(crate) types: BTreeSet<String>,
    pub(crate) methods: BTreeSet<String>,
}

impl Drop for Compiler {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn run(command: &mut Command) -> Result<String, String> {
    let Output {
        status,
        stdout,
        stderr,
    } = command.output().map_err(|e| format!("run .NET SDK: {e}"))?;
    if status.success() {
        String::from_utf8(stdout).map_err(|e| e.to_string())
    } else {
        Err(format!(
            ".NET command failed ({status})\n{}{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        ))
    }
}

#[derive(Default)]
pub(super) struct Session {
    compiler: Option<Compiler>,
    assemblies: Vec<(PathBuf, Metadata)>,
}

impl Session {
    pub(super) fn include(&mut self, source: &Path) -> Result<(), String> {
        if self.compiler.is_none() {
            self.compiler = Some(Compiler::new()?);
        }
        let compiler = self.compiler.as_mut().unwrap();
        let assembly = compiler.compile(&[source.to_owned()])?;
        let metadata = compiler.scan(&assembly)?;
        self.assemblies.push((assembly, metadata));
        Ok(())
    }

    pub(super) fn invoke(&self, method: &str) -> Option<Result<String, String>> {
        let (assembly, _) = self
            .assemblies
            .iter()
            .rev()
            .find(|(_, metadata)| metadata.methods.contains(method))?;
        Some(self.compiler.as_ref().unwrap().invoke(assembly, method))
    }
}
