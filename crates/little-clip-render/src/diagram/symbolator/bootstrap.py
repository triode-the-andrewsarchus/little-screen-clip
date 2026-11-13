"""Canonical Symbolator setup, data-only interface extraction and in-memory SVG."""
def clean_source(source, verilog):
    comments = r'//[^\n]*' if verilog else r'--[^\n]*'
    pattern = r'/\*[\s\S]*?\*/|' + comments + r'|"(?:\\.|""|[^"])*"|\'[^\n\']\''
    return re.sub(pattern, lambda match: re.sub(r'[^\n]', ' ', match.group()), source)


def validate_source(source, verilog):
    clean = clean_source(source, verilog)
    if '/*' in clean or '*/' in clean or '"' in clean:
        raise ValueError('Unterminated HDL string or block comment')
    if re.search(r'`|\bimport\s', clean):
        raise ValueError('HDL includes, macros and imports are disabled in local previews')
    pending = []
    pairs = {')': '(', ']': '[', '}': '{'}
    for match in re.finditer(r'[()\[\]{}]', clean):
        token = match.group()
        if token in '([{':
            pending.append(token)
        elif not pending or pending.pop() != pairs[token]:
            raise ValueError('Unbalanced HDL delimiter at line ' + str(source.count('\n', 0, match.start()) + 1))
    if pending:
        raise ValueError('Unclosed HDL interface delimiter')


def validate_interfaces(objects):
    if not 1 <= len(objects) <= 16:
        raise ValueError('Symbolator previews require between one and sixteen HDL interfaces')
    for component in objects:
        pins = list(component.generics) + list(component.ports)
        if not pins or len(pins) > 512:
            raise ValueError('HDL interfaces require between one and 512 ports or parameters')
        for pin in pins:
            if pin.mode not in ('in', 'out', 'inout', 'buffer', 'input', 'output') or pin.data_type is None:
                raise ValueError('Incomplete or unsupported HDL interface declaration: ' + pin.name)


def create_canvas(surface):
    from nucanvas import NuCanvas
    from nucanvas.shapes import PathShape, OvalShape
    canvas = NuCanvas(surface)
    for name, shape, reference in [
            ('arrow_fwd', PathShape(((0, -4), (2, -1, 2, 1, 0, 4), (8, 0), 'z'), fill=(0, 0, 0), weight=0), (3.2, 0)),
            ('arrow_back', PathShape(((0, -4), (-2, -1, -2, 1, 0, 4), (-8, 0), 'z'), fill=(0, 0, 0), weight=0), (-3.2, 0)),
            ('bubble', OvalShape(-3, -3, 3, 3, fill=(255, 255, 255), weight=1), (0, 0)),
            ('clock', PathShape(((0, -7), (0, 7), (7, 0), 'z'), fill=(255, 255, 255), weight=1), (0, 0))]:
        canvas.add_marker(name, shape, reference, 'auto', None)
    return canvas


def prepare(fonts):
    from nucanvas import DrawStyle, NuCanvas
    from nucanvas.shapes import PathShape, OvalShape
    from nucanvas.svg_backend import SvgSurface
    from symbolator import symbolator
    from hdlparse import vhdl_parser as vhdl, verilog_parser as verilog

    def text_bbox(self, text, params, spacing=0):
        family, size, style = params
        # Canonical SVG uses point sizes; Pillow measures physical CSS pixels.
        face = fonts.face(family, size * 96 / 72, style)
        plain = ''.join(ET.fromstring('<text>' + text + '</text>').itertext())
        lines = plain.split('\n')
        ascent, descent = face.getmetrics()
        width = max(face.getlength(line) for line in lines)
        height = len(lines) * (ascent + descent) + max(0, len(lines) - 1) * spacing
        return [0, 0, width, height, ascent]

    SvgSurface.text_bbox = text_bbox
    # Prime ElementTree's lazy output codec before resource denial.
    SvgSurface.convert_pango_markup('')
    # File/library/cache entrypoints remain unavailable even if upstream changes
    # accidentally route a source-only extraction through one in the future.
    for parser in (vhdl, verilog):
        for name in ('parse_vhdl_file', 'parse_verilog_file'):
            if hasattr(parser, name):
                setattr(parser, name, denied)
    for extractor in (vhdl.VhdlExtractor, verilog.VerilogExtractor):
        for name in ('extract_objects', 'register_array_types_from_sources',
                     'load_array_types', 'save_array_types'):
            if hasattr(extractor, name):
                setattr(extractor, name, denied)
    symbolator.file_search = denied
    symbolator.main = denied

    def render(request):
        source = request['source'].lstrip('\ufeff') + '\n'
        hint = re.sub(r'--[^\n]*', ' ', clean_source(source, True))
        is_verilog = re.search(r'\bmodule\s+\w+', hint) is not None
        validate_source(source, is_verilog)
        if is_verilog:
            extractor = verilog.VerilogExtractor()
            objects = extractor.extract_objects_from_source(source)
            events = verilog.VerilogLexer.run(source)
            starts = {'module'}
            ends = {'end_module'}
        else:
            extractor = vhdl.VhdlExtractor()
            objects = [item for item in extractor.extract_objects_from_source(source)
                       if isinstance(item, (vhdl.VhdlEntity, vhdl.VhdlComponent))]
            events = vhdl.VhdlLexer.run(source)
            starts = {'entity', 'component'}
            ends = {'end_entity', 'end_component'}
        # Use the actual canonical lexer events to detect incomplete declarations;
        # never silently drop a truncated final interface after a valid first one.
        balance = 0
        for position, action, groups in events:
            balance += int(action in starts) - int(action in ends)
        if balance:
            raise ValueError('Incomplete HDL interface declaration')
        validate_interfaces(objects)
        pages = []
        for component in objects:
            # Names/section titles are labels, never authored SVG/Pango elements.
            component.name = symbolator.xml_escape(component.name)
            component.sections = {index: symbolator.xml_escape(name)
                                  for index, name in component.sections.items()}
            style = DrawStyle()
            style.line_color = (0, 0, 0)
            stream = io.StringIO()
            surface = SvgSurface(stream, style, padding=8)
            canvas = create_canvas(surface)
            symbolator.make_symbol(component, extractor, title=True).draw(0, 0, canvas)
            canvas.render(False)
            pages.append(stream.getvalue())
        return json.dumps(pages)

    return render
