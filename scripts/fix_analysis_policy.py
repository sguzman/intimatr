from pathlib import Path

command_path = Path("src/command.rs")
text = command_path.read_text()
old = '''            AnalysisCommand::ResolveAddress { expression } => {
                let modules = self.analysis_modules()?;
                let address = AddressExpression::parse(&expression)?.resolve(&modules)?;
                Ok(AnalysisResult::Address {
                    expression,
                    address,
                })
            }'''
new = '''            AnalysisCommand::ResolveAddress { expression } => {
                let address = match AddressExpression::parse(&expression)? {
                    AddressExpression::Absolute { address } => address,
                    relative @ AddressExpression::ModuleOffset { .. } => {
                        self.require_memory_read()?;
                        let modules = self.analysis_modules()?;
                        relative.resolve(&modules)?
                    }
                };
                Ok(AnalysisResult::Address {
                    expression,
                    address,
                })
            }'''
if old not in text:
    raise SystemExit("ResolveAddress command block not found")
command_path.write_text(text.replace(old, new, 1))

test_path = Path("tests/analysis_command.rs")
test = test_path.read_text()
marker = '''#[test]
fn pointer_chain_and_batch_are_rpc_serializable_analysis_primitives() {'''
addition = '''#[test]
fn module_relative_resolution_respects_memory_read_policy() {
    let dispatcher = CommandDispatcher::new(
        FakeMemory::new(vec![0_u8; 64]),
        ScannerConfig::default(),
        PolicyConfig {
            allow_memory_read: false,
            ..PolicyConfig::default()
        },
        CommandLimits::default(),
    );

    let absolute = dispatcher
        .execute(Command::Analysis {
            request: AnalysisCommand::ResolveAddress {
                expression: format!("0x{:X}", BASE),
            },
        })
        .expect("absolute address resolution should not require memory access");
    assert!(matches!(absolute.result, CommandResult::Analysis { .. }));

    let error = dispatcher
        .execute(Command::Analysis {
            request: AnalysisCommand::ResolveAddress {
                expression: "ExampleGame.exe+0x10".to_owned(),
            },
        })
        .expect_err("module-relative resolution should honor memory-read policy");
    assert_eq!(error.code(), "policy_denied");
}

'''
if marker not in test:
    raise SystemExit("analysis integration test insertion marker not found")
test_path.write_text(test.replace(marker, addition + marker, 1))
