Summary: Tool to mirror releases from Github
Name: github-mirror-releases
Version: %(cat VERSION)
Release: 1%{dist}
License: MIT
Group: Development/Tools
Source0: %{name}-%{version}.tar.gz
BuildRoot: %{_tmppath}/%{name}-%{version}-%{release}-root

AutoReqProv: no

BuildRequires: gcc
BuildRequires: autoconf
BuildRequires: automake
BuildRequires: libtool
BuildRequires: openssl-devel
BuildRequires: llvm-devel
BuildRequires: clang

%description
%{summary}

Built by: %__hammer_user_name__ (%__hammer_user_login__)
From git commit: %__hammer_git_hash__ (%__hammer_git_ref__)

Build details: %__hammer_build_url__

%prep

%build
if [ -e VERSION ]; then
   sed -i -e "s/^package[.]version = .*/package.version = \"$(cat VERSION)\"/" Cargo.toml
fi
cargo build --release

%install
rm -rf %{buildroot}
%{__mkdir} -p %{buildroot}%{_bindir}

%{__install} -pD -m 755 target/release/github-mirror-releases %{buildroot}%{_bindir}/github-mirror-releases
%{__install} -pD -m 644 config.yaml %{buildroot}%{_sysconfdir}/github-mirror-releases.yaml.example

%clean
rm -rf %{buildroot}

%files
%defattr(-,root,root,-)
%{_bindir}/github-mirror-releases
%{_sysconfdir}/github-mirror-releases.yaml.example
