# Pgf Tikz previews

Local source and rendered views of the same diagrams.

## native-tikz.tikz

```tikz
\usetikzlibrary{arrows.meta,positioning,calc}
\definecolor{stageblue}{HTML}{295F98}
\definecolor{stagegold}{HTML}{DA9B42}
\def\radius{0.15}
\begin{tikzpicture}[>=Stealth,thick]
  \node[draw=stageblue,fill=stageblue!10,rounded corners,minimum width=2.3cm,minimum height=0.9cm] (source) at (0,0) {Source};
  \node[draw=stageblue,fill=stageblue!10,rounded corners,minimum width=2.3cm,minimum height=0.9cm] (layout) at (3.5,0) {Native layout};
  \node[draw=stagegold,fill=stagegold!15,rounded corners,minimum width=2.3cm,minimum height=0.9cm] (preview) at (7,0) {Preview};
  \draw[->,stageblue] (source) -- (layout);
  \draw[->,stageblue] (layout) -- (preview);
  \draw[->,stagegold,bend left=28] (preview.south) to node[below] {$x^2+y^2=r^2$} (source.south);
  \begin{scope}[shift={(2,-2)}]
    \foreach \x in {0,1,2,3} {
      \fill[stagegold] (\x,0) circle (\radius);
    }
    \draw[stageblue] (0,0) .. controls (1,1) and (2,-1) .. (3,0);
  \end{scope}
\end{tikzpicture}
```

## native-pgf.pgf

```pgf
\definecolor{stageblue}{HTML}{295F98}
\definecolor{stagegold}{HTML}{DA9B42}
\begin{pgfpicture}
  \pgfsetlinewidth{1.2pt}
  \pgfsetstrokecolor{stageblue}
  \pgfsetfillcolor{stageblue!10}
  \pgfpathrectangle{\pgfpoint{0cm}{0cm}}{\pgfpoint{6cm}{2cm}}
  \pgfusepath{fill,stroke}
  \pgfsetstrokecolor{stagegold}
  \pgfpathmoveto{\pgfpoint{0.5cm}{0.5cm}}
  \pgfpathcurveto{\pgfpoint{2cm}{2cm}}{\pgfpoint{4cm}{0cm}}{\pgfpoint{5.5cm}{1.5cm}}
  \pgfusepath{stroke}
  \pgfsetfillcolor{black}
  \pgftext[at={\pgfpoint{3cm}{1cm}}]{PGF paths and $\int_0^1 x\,dx$}
\end{pgfpicture}
```

## native-pgfplots.pgfplots

```pgfplots
\begin{tikzpicture}
\begin{axis}[
  width=10cm,height=6cm,
  xlabel={$x$},ylabel={Value},
  xmin=0,xmax=3,ymin=0,ymax=10,
  grid=major,legend pos=north west
]
  \addplot[blue,thick,domain=0:3,samples=41] {x*x};
  \addlegendentry{$x^2$}
  \addplot[orange,thick,mark=*] table[row sep=\\] {
    x y\\
    0 1\\
    1 2\\
    2 3\\
    3 4\\
  };
  \addlegendentry{Inline observations}
\end{axis}
\end{tikzpicture}
```
