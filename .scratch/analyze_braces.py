with open('src/fuse/mod.rs', 'r', encoding='utf-8') as f:
    text = f.read()

brace_level = 0
in_string = False
in_char = False
escape = False
in_raw_string = False

i = 0
while i < len(text):
    c = text[i]
    if escape:
        escape = False
    elif not in_raw_string and c == '\\':
        escape = True
    elif not in_string and not in_raw_string and c == "'" and not in_char:
        # this is naive for chars, but let's assume valid rust chars
        in_char = True
    elif in_char and c == "'" and not escape:
        in_char = False
    elif not in_string and not in_char and text[i:i+2] == 'r"':
        in_raw_string = True
        i += 1
    elif not in_string and not in_char and text[i:i+3] == 'r#"':
        in_raw_string = True
        i += 2
    elif in_raw_string and c == '"':
        # Need to check for hash if r#"
        if i+1 < len(text) and text[i+1] == '#':
            in_raw_string = False
            i += 1
        elif text[i-1:i+1] == 'r"':
            pass
        else:
            in_raw_string = False
    elif not in_char and not in_raw_string and c == '"':
        in_string = not in_string
    elif not in_string and not in_char and not in_raw_string:
        if c == '{': brace_level += 1
        elif c == '}':
            brace_level -= 1
            if brace_level < 0:
                print(f'Negative brace level at char {i}')
    i += 1

print(f'Final real brace level: {brace_level}')
