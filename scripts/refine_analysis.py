from pathlib import Path

analysis_path = Path("src/analysis.rs")
text = analysis_path.read_text()
old = '''        let mut split_index = None;
        for (index, character) in expression.char_indices().skip(1) {
            if character == '+' || character == '-' {
                split_index = Some((index, character));
                break;
            }
        }
        let Some((index, sign)) = split_index else {
            return Ok(Self::ModuleOffset {
                module: expression.to_owned(),
                offset: 0,
            });
        };'''
new = '''        let split_index = expression.char_indices().rev().find(|&(index, character)| {
            if index == 0 || !matches!(character, '+' | '-') {
                return false;
            }
            let offset_text = expression[index + character.len_utf8()..].trim();
            !offset_text.is_empty() && parse_unsigned(offset_text).is_ok()
        });
        let Some((index, sign)) = split_index else {
            return Ok(Self::ModuleOffset {
                module: expression.to_owned(),
                offset: 0,
            });
        };'''
if old not in text:
    raise SystemExit("address-expression split block not found")
text = text.replace(old, new, 1)

test_marker = '''    #[test]
    fn resolves_pointer_chain_using_little_endian_pointers() {'''
new_test = '''    #[test]
    fn module_relative_expression_allows_hyphenated_module_names() {
        let modules = vec![ModuleDescriptor {
            name: "game-client.dll".to_owned(),
            path: r"C:\\Games\\game-client.dll".to_owned(),
            base: 0x180000000,
            size: 0x100000,
        }];
        assert_eq!(
            AddressExpression::parse("game-client.dll+0x40")
                .unwrap()
                .resolve(&modules)
                .unwrap(),
            0x180000040
        );
        assert_eq!(
            AddressExpression::parse("game-client.dll")
                .unwrap()
                .resolve(&modules)
                .unwrap(),
            0x180000000
        );
    }

'''
if test_marker not in text:
    raise SystemExit("analysis test insertion marker not found")
text = text.replace(test_marker, new_test + test_marker, 1)
analysis_path.write_text(text)

command_path = Path("src/command.rs")
command = command_path.read_text()
old = '''                let template = SavedWatchTemplate {
                    name: name.clone(),
                    address: format!("0x{:X}", watch.address),
                    value_type: watch.value_type,
                    frozen: watch.frozen,
                };'''
new = '''                let modules = self.analysis_modules()?;
                let address = modules
                    .iter()
                    .find(|module| {
                        watch.address >= module.base
                            && watch.address < module.base.saturating_add(module.size)
                    })
                    .map_or_else(
                        || format!("0x{:X}", watch.address),
                        |module| {
                            format!(
                                "{}+0x{:X}",
                                module.name,
                                watch.address.saturating_sub(module.base)
                            )
                        },
                    );
                let template = SavedWatchTemplate {
                    name: name.clone(),
                    address,
                    value_type: watch.value_type,
                    frozen: watch.frozen,
                };'''
if old not in command:
    raise SystemExit("saved watch template block not found")
command_path.write_text(command.replace(old, new, 1))
