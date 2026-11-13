"""Canonical family setup and AST validation; policy lives in python/bootstrap.py."""
def check_tree(tree):
    from blockdiag.utils import unquote
    resources = {'icon', 'image', 'background', 'backgroundimage',
                 'font', 'fontpath', 'fontfile', 'fontmap',
                 'config', 'plugin', 'plugins', 'href', 'url', 'src', 'stylesheet'}
    pending = [(tree, 0)]
    count = 0
    while pending:
        item, depth = pending.pop()
        count += 1
        if count > 16384 or depth > 64:
            raise ValueError('Diagram exceeds the statement or nesting budget')
        if type(item).__name__ == 'Extension' and item.type == 'plugin':
            raise ValueError('Diagram plugins are disabled in local previews')
        if type(item).__name__ == 'Attr':
            name = unquote(item.name).lower()
            while name.startswith('default_'):
                name = name[len('default_'):]
            if name.replace('_', '') in resources:
                raise ValueError('External diagram resource attribute is disabled: ' + item.name)
            if name == 'fontsize':
                value = int(unquote(item.value))
                if not 1 <= value <= 128:
                    raise ValueError('Diagram font size must be between 1 and 128')
        if isinstance(item, (tuple, list)):
            pending.extend((child, depth + 1) for child in item)


def prepare(fonts):
    from blockdiag import imagedraw, noderenderer, plugins
    from blockdiag.imagedraw import png, svg
    from blockdiag.utils import images, uuid
    import webcolors
    import blockdiag.imagedraw.filters.linejump
    from blockdiag.utils.fontmap import FontMap

    # Load a fixed set of bundled shapes, never installed entry-point plugins.
    shapes = ('box', 'square', 'roundedbox', 'diamond', 'minidiamond', 'mail',
              'note', 'cloud', 'circle', 'ellipse', 'beginpoint', 'endpoint',
              'actor', 'flowchart.database', 'flowchart.input', 'flowchart.loopin',
              'flowchart.loopout', 'flowchart.terminator', 'textbox', 'dots', 'none')
    for name in shapes:
        module = importlib.import_module('blockdiag.noderenderer.' + name)
        module.setup(module)
    packet_shapes = importlib.import_module('packetdiag.noderenderers')
    packet_shapes.setup(packet_shapes)
    svg.setup(svg)
    noderenderer.init_renderers = denied
    imagedraw.init_imagedrawers = denied
    plugins.load = denied
    # Same switch as upstream's --no-external-images, plus failing resolvers.
    images.external_images = False
    images.open = denied
    images.urlopen = denied
    svg.SVGImageDrawElement.image = denied
    # Anonymous diagram identities need no host MAC/time discovery from uuid1.
    identities = itertools.count()
    uuid.generate = lambda: 'clip-node-' + str(next(identities))

    def bundled_font(font):
        if font.path is not None:
            denied()
        return fonts.face(font.generic_family, font.size, font.weight + ' ' + font.style)

    # Measures labels with the same bundled metric faces used by compose_svg.
    png.ttfont_for = bundled_font
    modules = {flavor: (
        importlib.import_module(flavor + '.parser'),
        importlib.import_module(flavor + '.builder'),
        importlib.import_module(flavor + '.drawer')) for flavor in
        ('blockdiag', 'seqdiag', 'actdiag', 'nwdiag', 'packetdiag', 'rackdiag')}

    def render(request):
        flavor = request['flavor']
        if flavor not in modules:
            raise ValueError('Unknown BlockDiag family language')
        parser, builder, drawer = modules[flavor]
        tree = parser.parse_string(request['source'].lstrip('\ufeff'))
        check_tree(tree)
        model = builder.ScreenNodeBuilder.build(tree)
        drawing = drawer.DiagramDraw('SVG', model, None, fontmap=FontMap(),
                                     nodoctype=True, code=None)
        drawing.draw()
        return drawing.save()

    return render
