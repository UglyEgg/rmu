%global __brp_strip %{nil}
%global __brp_strip_static_archive %{nil}
%global __brp_strip_comment_note %{nil}
%global __brp_remove_la_files %{nil}

Name:           rmu
Version:        %{rmu_version}
Release:        %{rmu_release}
Summary:        Recoverable Move Utility with a GNU rm-compatible interface
License:        MIT
URL:            https://github.com/UglyEgg/rmu
BuildArch:      %{rmu_arch}
AutoReqProv:    no
Requires:       coreutils

%description
RMU moves files into the FreeDesktop Trash by default while preserving GNU
rm's documented command-line decisions. Explicit delegate mode hands off to
GNU coreutils rm. rmuctl lists, restores, purges, empties, and safely
diagnoses the same Trash without a daemon or private index. It also prints
completion definitions for custom shell configurations.

%prep

%build

%install
install -D -m 0755 /input/rmu %{buildroot}%{_bindir}/rmu
install -D -m 0755 /input/rmuctl %{buildroot}%{_bindir}/rmuctl
install -D -m 0644 /input/man/rmu.1 %{buildroot}%{_mandir}/man1/rmu.1
install -D -m 0644 /input/man/rmuctl.1 %{buildroot}%{_mandir}/man1/rmuctl.1
gzip -n -9 %{buildroot}%{_mandir}/man1/rmu.1
gzip -n -9 %{buildroot}%{_mandir}/man1/rmuctl.1
install -D -m 0644 /input/completions/rmu.bash %{buildroot}%{_datadir}/bash-completion/completions/rmu
install -D -m 0644 /input/completions/rmuctl.bash %{buildroot}%{_datadir}/bash-completion/completions/rmuctl
install -D -m 0644 /input/completions/_rmu %{buildroot}%{_datadir}/zsh/site-functions/_rmu
install -D -m 0644 /input/completions/_rmuctl %{buildroot}%{_datadir}/zsh/site-functions/_rmuctl
install -D -m 0644 /input/completions/rmu.fish %{buildroot}%{_datadir}/fish/vendor_completions.d/rmu.fish
install -D -m 0644 /input/completions/rmuctl.fish %{buildroot}%{_datadir}/fish/vendor_completions.d/rmuctl.fish
install -D -m 0644 /input/LICENSE %{buildroot}%{_licensedir}/rmu/LICENSE
install -D -m 0644 /input/licenses/Rust-%{rust_version}-COPYRIGHT %{buildroot}%{_licensedir}/rmu/Rust-%{rust_version}-COPYRIGHT
install -D -m 0644 /input/licenses/Rust-%{rust_version}-COPYRIGHT-library.html %{buildroot}%{_licensedir}/rmu/Rust-%{rust_version}-COPYRIGHT-library.html
install -D -m 0644 /input/licenses/Rust-%{rust_version}-LICENSE-MIT %{buildroot}%{_licensedir}/rmu/Rust-%{rust_version}-LICENSE-MIT
install -D -m 0644 /input/licenses/musl-1.2.5-COPYRIGHT %{buildroot}%{_licensedir}/rmu/musl-1.2.5-COPYRIGHT
install -D -m 0644 /input/README.md %{buildroot}%{_docdir}/rmu/README.md
install -D -m 0644 /input/CHANGELOG.md %{buildroot}%{_docdir}/rmu/CHANGELOG.md
install -D -m 0644 /input/UPSTREAM-NOTICES.md %{buildroot}%{_docdir}/rmu/UPSTREAM-NOTICES.md

%files
%{_bindir}/rmu
%{_bindir}/rmuctl
%{_mandir}/man1/rmu.1.gz
%{_mandir}/man1/rmuctl.1.gz
%{_datadir}/bash-completion/completions/rmu
%{_datadir}/bash-completion/completions/rmuctl
%{_datadir}/zsh/site-functions/_rmu
%{_datadir}/zsh/site-functions/_rmuctl
%{_datadir}/fish/vendor_completions.d/rmu.fish
%{_datadir}/fish/vendor_completions.d/rmuctl.fish
%dir %{_licensedir}/rmu
%license %{_licensedir}/rmu/LICENSE
%license %{_licensedir}/rmu/Rust-%{rust_version}-COPYRIGHT
%license %{_licensedir}/rmu/Rust-%{rust_version}-COPYRIGHT-library.html
%license %{_licensedir}/rmu/Rust-%{rust_version}-LICENSE-MIT
%license %{_licensedir}/rmu/musl-1.2.5-COPYRIGHT
%doc %{_docdir}/rmu/README.md
%doc %{_docdir}/rmu/CHANGELOG.md
%doc %{_docdir}/rmu/UPSTREAM-NOTICES.md

%changelog
* Tue Sep 08 2026 RMU contributors - %{rmu_version}-%{rmu_release}
- Package the static commands, manuals, and shell completions.
