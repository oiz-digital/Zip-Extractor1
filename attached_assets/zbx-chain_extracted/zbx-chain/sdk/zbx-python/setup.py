from setuptools import setup, find_packages

setup(
    name="zbx-chain",
    version="1.0.0",
    description="Zebvix Chain Python SDK — client, wallet, staking, bridge, and oracle utilities.",
    long_description=open("README.md").read(),
    long_description_content_type="text/markdown",
    author="Zebvix Foundation",
    author_email="dev@zebvix.com",
    url="https://github.com/zebvix/zbx-python",
    license="Apache-2.0",
    packages=find_packages(where="src"),
    package_dir={"": "src"},
    python_requires=">=3.10",
    install_requires=[
        "httpx>=0.27",
        "eth-account>=0.11",
        "eth-typing>=4",
        "hexbytes>=1",
        "pydantic>=2",
        "websockets>=12",
    ],
    extras_require={
        "dev": ["pytest>=8", "pytest-asyncio>=0.23", "respx>=0.21"],
    },
    classifiers=[
        "Programming Language :: Python :: 3",
        "License :: OSI Approved :: Apache Software License",
        "Intended Audience :: Developers",
        "Topic :: Software Development :: Libraries :: Python Modules",
    ],
)
