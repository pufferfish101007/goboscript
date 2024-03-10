use std::{
    collections::{HashMap, HashSet},
    env,
    fs::{self, File},
    io::{self, BufWriter},
    path::PathBuf,
    time::Instant,
};

use colored::*;
use logos::Span;

use crate::{
    ast::{Declrs, Names},
    codegen::CodeGen,
    config::Config,
    grammar::DeclrsParser,
    logoslalrpop::Lexer,
    reporting::{Report, ReportLevel, Summary},
    visitors::Visitor,
    zipfile::ZipFile,
};

#[derive(Clone)]
pub struct FunctionPrototype<'src> {
    pub args: Names<'src>,
    pub args_set: HashSet<&'src str>,
    pub warp: bool,
    pub span: Span,
}

pub struct Program<'src> {
    pub declrs: Declrs<'src>,
    pub variables: HashSet<&'src str>,
    pub lists: HashSet<&'src str>,
    pub functions: HashMap<&'src str, FunctionPrototype<'src>>,
}

pub struct Sprite<'src> {
    pub program: Option<Program<'src>>,
    pub reports: Vec<Report<'src>>,
}

pub fn build(input: Option<PathBuf>, output: Option<PathBuf>) -> io::Result<()> {
    let before = Instant::now();
    let mut summary = Summary::new();
    let input = match input {
        Some(input) => input,
        None => env::current_dir()?,
    };
    let project_name = input.file_name().unwrap().to_str().unwrap();
    let output = match output {
        Some(output) => output,
        None => input.join(project_name).with_extension("sb3"),
    };
    let config: Config = match fs::read_to_string(input.join("goboscript.toml")) {
        Ok(config) => match toml::from_str(&config) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("{}: {}", "Error".red().bold(), err);
                return Ok(());
            }
        },
        Err(_) => Default::default(),
    };
    let stage_path = input.join("stage").with_extension("gs");
    let stage_src = fs::read_to_string(&stage_path)?;
    let lexer = Lexer::new(&stage_src);
    let parser = DeclrsParser::new();
    let mut stage = match parser.parse(&stage_src, lexer) {
        Ok(mut declrs) => {
            let mut variables = HashSet::new();
            let mut lists = HashSet::new();
            let mut functions = HashMap::new();
            let mut reports = Vec::new();
            let mut visitor = Visitor {
                variables: &mut variables,
                lists: &mut lists,
                functions: &mut functions,
                reports: &mut reports,
            };
            visitor.visit_declrs(&mut declrs);
            Sprite {
                program: Some(Program { declrs, variables, lists, functions }),
                reports,
            }
        }
        Err(err) => {
            let report = Report::ParserError(err);
            Sprite { program: None, reports: vec![report] }
        }
    };
    let mut srcs: Vec<(PathBuf, String)> = Vec::new();
    let mut sprites: Vec<Sprite> = Vec::new();
    for entry in fs::read_dir(&input)? {
        let path = entry?.path();
        if !path.is_file()
            || path.extension() != Some("gs".as_ref())
            || path.file_stem() == Some("stage".as_ref())
        {
            continue;
        }
        let src = fs::read_to_string(&path)?;
        srcs.push((path, src));
    }
    for (_path, src) in &srcs {
        let mut reports: Vec<Report> = Vec::new();
        let lexer = Lexer::new(src);
        let parser = DeclrsParser::new();
        let program = match parser.parse(src, lexer) {
            Ok(mut declrs) => {
                let mut variables = HashSet::new();
                let mut lists = HashSet::new();
                let mut functions = HashMap::new();
                let mut visitor = Visitor {
                    variables: &mut variables,
                    lists: &mut lists,
                    functions: &mut functions,
                    reports: &mut reports,
                };
                visitor.visit_declrs(&mut declrs);
                Some(Program { declrs, variables, lists, functions })
            }
            Err(err) => {
                reports.push(Report::ParserError(err));
                None
            }
        };
        sprites.push(Sprite { program, reports });
    }
    let mut codegen = CodeGen::new(
        ZipFile::new(BufWriter::new(File::create(output)?)),
        input,
        config,
    );
    codegen.begin_project()?;
    if let Some(program) = &stage.program {
        codegen.sprite("Stage", program, None, None, &mut stage.reports, false)?;
    }
    for ((path, src), sprite) in srcs.iter().zip(sprites.iter_mut()) {
        let name = path.file_stem().unwrap().to_str().unwrap();
        if let Some(program) = &sprite.program {
            codegen.sprite(
                name,
                program,
                stage.program.as_ref().map(|program| &program.variables),
                stage.program.as_ref().map(|program| &program.lists),
                &mut sprite.reports,
                true,
            )?;
        }
        for report in &sprite.reports {
            if matches!(report.level(), ReportLevel::Warning) {
                report.print(path.to_str().unwrap(), src);
            }
        }
        summary.summarize(&sprite.reports);
    }
    for report in &stage.reports {
        if matches!(report.level(), ReportLevel::Warning) {
            report.print(stage_path.to_str().unwrap(), &stage_src);
        }
    }
    for ((path, src), sprite) in srcs.iter().zip(sprites) {
        for report in &sprite.reports {
            if matches!(report.level(), ReportLevel::Error) {
                report.print(path.to_str().unwrap(), src);
            }
        }
    }
    for report in &stage.reports {
        if matches!(report.level(), ReportLevel::Error) {
            report.print(stage_path.to_str().unwrap(), &stage_src);
        }
    }
    summary.summarize(&stage.reports);
    codegen.end_project()?;
    if summary.warnings > 0 {
        eprintln!(
            "{} {}",
            summary.warnings.to_string().bold(),
            (if summary.warnings == 1 { "warning found" } else { "warnings found" })
                .yellow()
                .bold()
        );
    }
    if summary.errors > 0 {
        eprintln!(
            "{} {}",
            summary.errors.to_string().bold(),
            (if summary.errors == 1 { "error found" } else { "errors found" })
                .red()
                .bold()
        );
    }
    eprintln!("{} in {:#?}", "Finished".green().bold(), before.elapsed());
    Ok(())
}